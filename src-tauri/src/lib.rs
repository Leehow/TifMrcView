#[cfg(all(feature = "desktop", target_os = "android"))]
mod android;
#[cfg(feature = "desktop")]
mod assoc;
pub mod align;
pub mod ctf;
pub mod denoise;
mod denoise_nad;
mod nad_eed;
#[cfg(feature = "pro")]
mod nad_eed_gpu;
#[cfg(feature = "pro")]
pub mod model_store;
#[cfg(feature = "pro")]
pub mod trained_models;
pub mod diffraction;
#[cfg(feature = "pro")]
pub mod ctf_gpu;
#[cfg(feature = "pro")]
pub mod denoise_gpu;
#[cfg(feature = "pro")]
pub mod denoise_learn;
pub mod edition;
#[cfg_attr(not(all(feature = "desktop", feature = "pro")), allow(dead_code))]
mod documents;
#[cfg_attr(not(all(feature = "desktop", feature = "pro")), allow(dead_code))]
mod merge_export;
#[cfg_attr(not(all(feature = "desktop", feature = "pro")), allow(dead_code))]
mod merge_project;
pub mod fft;
pub mod formats;
pub mod imod;
pub mod labels;
pub mod surfaces;
#[cfg(feature = "pro")]
pub mod classify;
#[cfg(feature = "pro")]
pub mod unet;
#[cfg(feature = "pro")]
pub mod wedge;
#[cfg(feature = "pro")]
pub mod membrane;
#[cfg(feature = "pro")]
pub mod filaments;
#[cfg(feature = "pro")]
pub mod live;
pub mod image;
pub mod ortho;
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod open_requests;
#[cfg(feature = "desktop")]
mod mem;
#[cfg(all(test, feature = "desktop"))]
mod ipc_config_tests;
pub mod plugins;
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod plugin_assoc;
pub mod render;
pub mod relocate;
pub mod reslice;
pub mod spectro;
pub mod particles;
pub mod stem4d;
pub mod spectrum;
pub mod volume;
pub mod xray_data;
#[cfg(all(feature = "desktop", not(target_os = "android")))]
mod update;
#[cfg(all(feature = "desktop", target_os = "android"))]
#[path = "update_android.rs"]
mod update;
#[cfg(any(feature = "desktop", feature = "pro"))]
pub mod sam;
#[cfg(any(feature = "desktop", feature = "pro"))]
pub mod segpick;
pub mod thumb;

#[cfg(feature = "desktop")]
include!("desktop.rs");
#[cfg(all(feature = "desktop", feature = "pro"))]
include!("desktop_documents.rs");

#[cfg(feature = "pro")]
pub mod project_dirs;

#[cfg(feature = "pro")]
pub mod tomo;

#[cfg(feature = "pro")]
pub mod motion;

#[cfg(feature = "pro")]
pub mod spa;

/// Subtomogram averaging (Pro).
#[cfg(feature = "pro")]
pub mod sta;

/// The headless command-line tool `tmv` (Pro processing on servers).
#[cfg(feature = "pro")]
pub mod cli;
