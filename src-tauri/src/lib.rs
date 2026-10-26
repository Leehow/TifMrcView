#[cfg(feature = "desktop")]
mod assoc;
pub mod formats;
pub mod imod;
pub mod image;
#[cfg(feature = "desktop")]
mod mem;
#[cfg(all(test, feature = "desktop"))]
mod ipc_config_tests;
pub mod plugins;
pub mod render;
pub mod reslice;
#[cfg(feature = "desktop")]
mod update;
pub mod thumb;

#[cfg(feature = "desktop")]
include!("desktop.rs");
