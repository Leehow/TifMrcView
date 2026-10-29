#[cfg(all(feature = "desktop", target_os = "android"))]
mod android;
#[cfg(feature = "desktop")]
mod assoc;
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
#[cfg(all(feature = "desktop", not(target_os = "android")))]
mod update;
#[cfg(all(feature = "desktop", target_os = "android"))]
#[path = "update_android.rs"]
mod update;
#[cfg(feature = "desktop")]
pub mod sam;
pub mod thumb;

#[cfg(feature = "desktop")]
include!("desktop.rs");
