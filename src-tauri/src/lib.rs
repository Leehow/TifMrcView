pub mod align;
#[cfg(all(feature = "desktop", target_os = "android"))]
mod android;
#[cfg(feature = "desktop")]
mod assoc;
#[cfg(feature = "pro")]
pub mod classify;
pub mod ctf;
#[cfg(feature = "pro")]
pub mod ctf_gpu;
pub mod denoise;
#[cfg(feature = "pro")]
pub mod denoise_gpu;
#[cfg(feature = "pro")]
pub mod denoise_learn;
pub mod diffraction;
pub mod edition;
pub mod fft;
pub mod formats;
pub mod image;
pub mod imod;
#[cfg(all(test, feature = "desktop"))]
mod ipc_config_tests;
pub mod labels;
#[cfg(feature = "desktop")]
mod mem;
pub mod particles;
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod plugin_assoc;
pub mod plugins;
pub mod render;
pub mod reslice;
#[cfg(any(feature = "desktop", feature = "pro"))]
pub mod sam;
pub mod spectro;
pub mod spectrum;
pub mod stem4d;
pub mod thumb;
#[cfg(feature = "pro")]
pub mod unet;
#[cfg(all(feature = "desktop", not(target_os = "android")))]
mod update;
#[cfg(all(feature = "desktop", target_os = "android"))]
#[path = "update_android.rs"]
mod update;
pub mod volume;
#[cfg(feature = "pro")]
pub mod wedge;
pub mod xray_data;

#[cfg(feature = "desktop")]
include!("desktop.rs");

#[cfg(feature = "pro")]
pub mod tomo;

#[cfg(feature = "pro")]
pub mod motion;

#[cfg(feature = "pro")]
pub mod spa;
