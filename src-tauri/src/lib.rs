#[cfg(feature = "desktop")]
mod assoc;
pub mod ctf;
#[cfg(feature = "pro")]
pub mod ctf_gpu;
pub mod edition;
pub mod fft;
pub mod formats;
pub mod imod;
pub mod labels;
pub mod image;
#[cfg(feature = "desktop")]
mod mem;
#[cfg(all(test, feature = "desktop"))]
mod ipc_config_tests;
pub mod plugins;
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod plugin_assoc;
pub mod render;
pub mod reslice;
pub mod spectrum;
pub mod volume;
#[cfg(feature = "desktop")]
mod update;
#[cfg(feature = "desktop")]
pub mod sam;
pub mod thumb;

#[cfg(feature = "desktop")]
include!("desktop.rs");
