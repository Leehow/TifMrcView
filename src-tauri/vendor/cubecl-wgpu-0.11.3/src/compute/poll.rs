#[cfg(not(target_family = "wasm"))]
mod _impl {
    use std::thread::JoinHandle;

    #[derive(Debug)]
    pub struct WgpuPoll {
        active_handle: std::sync::Arc<()>,
        cancel_sender: std::sync::mpsc::Sender<()>,
        poll_thread: JoinHandle<()>,
    }

    impl WgpuPoll {
        pub fn new(device: wgpu::Device, poison: super::super::device_poison::PoisonWatch) -> Self {
            let active_handle = std::sync::Arc::new(());
            let thread_check = active_handle.clone();

            let (cancel_sender, cancel_receiver) = std::sync::mpsc::channel();
            let poll_thread = std::thread::spawn(move || {
                loop {
                    // Check whether the WgpuPoll, this thread, and something else is holding
                    // a handle.
                    if std::sync::Arc::strong_count(&thread_check) > 2 {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.poll(wgpu::PollType::Wait {
                            submission_index: None, // Wait for most recent
                            timeout: None,
                        })));
                        match result {
                            Ok(Err(e)) => log::warn!(
                                "wgpu: requested wait timed out before the submission was completed during sync. ({e})"
                            ),
                            Err(_) => {
                                poison.fail("wgpu polling failed before the map callback completed".into());
                                break;
                            }
                            Ok(Ok(_)) => {}
                        }
                        if poison.is_poisoned() {
                            break;
                        }
                    } else {
                        // Do not cancel thread while someone still needs to poll.
                        if cancel_receiver.try_recv().is_ok() {
                            break;
                        }

                        std::thread::park();
                    }
                    std::thread::yield_now();
                }
            });

            Self {
                active_handle,
                cancel_sender,
                poll_thread,
            }
        }
        /// Get a handle, as long as it's alive the polling will be active.
        pub fn start_polling(&self) -> std::sync::Arc<()> {
            let handle = self.active_handle.clone();
            self.poll_thread.thread().unpark();
            handle
        }
    }

    impl Drop for WgpuPoll {
        fn drop(&mut self) {
            // A lost device may already have stopped the polling thread.
            let _ = self.cancel_sender.send(());
            self.poll_thread.thread().unpark();
        }
    }
}

// On Wasm, the browser handles the polling loop, so we don't need anything.
#[cfg(target_family = "wasm")]
mod _impl {
    #[derive(Debug)]
    pub struct WgpuPoll {}
    impl WgpuPoll {
        pub fn new(_device: wgpu::Device, _poison: super::super::device_poison::PoisonWatch) -> Self {
            Self {}
        }
        pub fn start_polling(&self) -> alloc::sync::Arc<()> {
            alloc::sync::Arc::new(())
        }
    }
}

pub(crate) use _impl::*;
