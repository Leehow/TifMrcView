//! Subtomogram averaging (Pro): many copies of one particle, found in
//! tomograms, are brought into register and averaged, so the noise of each
//! copy cancels and the missing wedge of one copy is filled by copies lying
//! at other orientations. Independent Rust implementations of published
//! methods (see each module); no translated software.
//!
//! The work is split between two engines:
//!
//! - 3D (`cc`, `average`, `pick`): small cubes cut from a low-resolution
//!   tomogram, compared by cross-correlation restricted to the Fourier
//!   components each cube really holds (constrained cross-correlation,
//!   Förster et al., PNAS 102, 2005) and averaged with the same weights.
//!   It finds particles, makes a first model and aligns coarsely.
//! - 2D (later stages): each particle's own set of tilt images, refined
//!   through the single-particle code (`crate::spa`), as RELION 5 and M do.
//!
//! Conventions. Volumes are n³ cubes in z, y, x order with the centre at
//! voxel n/2 (the phase origin of `spa::fourier`). The tilt axis is Y and
//! the beam is Z, as in the tomograms `crate::tomo` writes. A pose (R, t)
//! says where the particle is: particle(x) = reference(Rᵀ·(x − c − t) + c),
//! the reference turned by R about the centre c and then moved by t voxels.

pub mod alike;
pub mod auto;
pub mod average;
pub mod cc;
pub mod cc_gpu;
pub mod filaments;
pub mod flat;
pub mod gallery;
pub mod kinds;
pub mod marks;
pub mod membranes;
pub mod near;
pub mod own_map;
pub mod paint;
pub mod pick;
pub mod placed;
pub mod pits;
pub mod project;
pub mod refine;
pub mod review;
pub mod search;
pub mod search_gpu;
pub mod shapes;
pub mod shape;
pub mod sheet;
pub mod taught;
pub mod tilt;
#[cfg(test)]
mod real;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};

use crate::spa::geom::Mat;

pub static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::Relaxed) {
        bail!("已取消");
    }
    Ok(())
}

/// Where a particle is relative to the reference (see the module text).
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub r: Mat,
    /// Shift in voxels, (x, y, z).
    pub t: [f64; 3],
    /// Normalised cross-correlation with the reference at this pose.
    pub score: f32,
}

pub const IDENTITY: Mat = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// A turn about the Z axis.
pub(crate) fn cc_axis(deg: f64) -> Mat {
    crate::spa::geom::axis_angle([0.0, 0.0, 1.0], deg)
}
