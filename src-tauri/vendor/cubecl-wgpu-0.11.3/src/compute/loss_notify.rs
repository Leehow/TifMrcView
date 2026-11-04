//! Wake pending readbacks when the device fails, even without a map callback.

use std::sync::{Arc, Mutex, Weak};

pub(super) type Listener = dyn Fn() + Send + Sync;

#[derive(Default)]
struct State {
    lost: bool,
    listeners: Vec<Weak<Listener>>,
}

#[derive(Default)]
pub(super) struct LossNotify(Mutex<State>);

impl core::fmt::Debug for LossNotify {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LossNotify").finish_non_exhaustive()
    }
}

impl LossNotify {
    /// Retain the returned guard while waiting. Completed reads leave only a
    /// weak entry, pruned on the next registration rather than growing forever.
    pub fn subscribe(&self, notify: impl Fn() + Send + Sync + 'static) -> Arc<Listener> {
        let listener: Arc<Listener> = Arc::new(notify);
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.lost {
            drop(state);
            listener();
        } else {
            state.listeners.retain(|l| l.strong_count() > 0);
            state.listeners.push(Arc::downgrade(&listener));
        }
        listener
    }

    pub fn notify_all(&self) {
        let listeners = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.lost = true;
            core::mem::take(&mut state.listeners)
        };
        for listener in listeners.into_iter().filter_map(|l| l.upgrade()) {
            listener();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn loss_and_registration_cannot_miss_each_other() {
        for _ in 0..64 {
            let loss = Arc::new(LossNotify::default());
            let (send, recv) = mpsc::channel();
            let other = loss.clone();
            let thread = std::thread::spawn(move || other.notify_all());
            let _guard = loss.subscribe(move || { let _ = send.send(()); });
            thread.join().unwrap();
            recv.recv_timeout(Duration::from_secs(1)).expect("lost device must wake the read");
        }
    }

    #[test]
    fn already_lost_device_wakes_new_reads_immediately() {
        let loss = LossNotify::default();
        loss.notify_all();
        let (send, recv) = mpsc::channel();
        let _guard = loss.subscribe(move || { let _ = send.send(()); });
        recv.try_recv().unwrap();
    }

    #[test]
    fn completed_read_does_not_keep_its_callback_alive() {
        let loss = LossNotify::default();
        let guard = loss.subscribe(|| panic!("completed read must not be notified"));
        drop(guard);
        loss.notify_all();
    }
}
