//! Monitoring wgpu device poisoning.
//!
//! wgpu bounds-checks buffer accesses, so a kernel cannot fault the device the
//! way an illegal address faults a CUDA context. What remains is losing the
//! device outright — a driver reset, a timeout, a GPU that went away — which
//! wgpu reports once, through the device-lost callback, and never on the
//! operations that follow.
//!
//! The device is marked poisoned here when the callback fires, shared by every
//! stream on the device, and every sync point checks it

use cubecl_server::driver::DevicePoison;
use cubecl_server::server::ServerError;
use std::sync::{Arc, OnceLock};

/// Holds whether a wgpu device is poisoned. Shared by every stream on the device.
#[derive(Clone, Debug, Default)]
pub struct PoisonWatch {
    reason: Arc<OnceLock<String>>,
    listeners: Arc<super::loss_notify::LossNotify>,
}

impl PoisonWatch {
    /// Start watching `device`, installing the callbacks that poison it once wgpu loses it.
    ///
    /// Installs an uncaptured-error handler too. wgpu's default one panics,
    /// which on a healthy device is what surfaces a validation bug, so it
    /// still does; but once the device is poisoned every call on it errors.
    pub fn watch(device: &wgpu::Device) -> Self {
        let poison = Self::default();

        let failed = poison.clone();
        device.set_device_lost_callback(move |kind, message| {
            failed.fail(format!("{kind:?}: {message}"));
            log::error!("the wgpu device was lost ({kind:?}): {message}");
        });

        let reason = poison.reason.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            if reason.get().is_some() {
                log::debug!("error on a poisoned wgpu device: {error}");
            } else {
                panic!("wgpu error: {error}");
            }
        }));

        poison
    }

    pub(super) fn fail(&self, message: String) {
        // Publish the reason before waking a read which will call check().
        let _ = self.reason.set(message);
        self.listeners.notify_all();
    }

    pub(super) fn watch_map<T: Send + 'static>(
        &self,
        sender: cubecl_environment::future::channel::Sender<T>,
    ) -> Arc<super::loss_notify::Listener> {
        self.listeners.subscribe(move || { sender.close(); })
    }

    /// Whether the device is poisoned.
    pub fn is_poisoned(&self) -> bool {
        self.reason.get().is_some()
    }

    /// Returns a [ServerError::DevicePoisoned](ServerError::DevicePoisoned) once the device is poisoned.
    pub fn check(&self) -> Result<(), ServerError> {
        match self.reason.get() {
            Some(reason) => Err(DevicePoison::new(reason.clone()).into()),
            None => Ok(()),
        }
    }
}
