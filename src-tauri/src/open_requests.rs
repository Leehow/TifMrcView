//! Opening a file may finish after a later open, or after its window closes.
//! Keep the ownership check and the commit in one short critical section;
//! decoding and prefetching run outside it.
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct OpenRequests(Mutex<HashMap<String, u64>>);

impl OpenRequests {
    pub fn begin(&self, window: &str, token: u64) -> bool {
        let mut requests = self.0.lock().unwrap();
        let latest = requests.entry(window.into()).or_default();
        if token <= *latest {
            return false;
        }
        *latest = token;
        true
    }

    pub fn commit<T>(&self, window: &str, token: u64, apply: impl FnOnce() -> T) -> Result<T, &'static str> {
        let requests = self.0.lock().unwrap();
        if requests.get(window) != Some(&token) {
            return Err("stale");
        }
        Ok(apply())
    }

    pub fn close(&self, window: &str) {
        self.0.lock().unwrap().remove(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_open_can_replace_a_windows_image() {
        let r = OpenRequests::default();
        let mut current = 0;
        r.begin("one", 1);
        r.begin("one", 2);
        r.commit("one", 2, || current = 2).unwrap();
        assert!(r.commit("one", 1, || current = 1).is_err());
        assert_eq!(current, 2);
    }

    #[test]
    fn a_failed_new_open_does_not_reenable_an_older_open() {
        let r = OpenRequests::default();
        r.begin("one", 1);
        r.begin("one", 2); // This decoding fails, so it never commits.
        assert!(r.commit("one", 1, || panic!("old image committed")).is_err());
    }

    #[test]
    fn closing_a_window_rejects_its_pending_open() {
        let r = OpenRequests::default();
        r.begin("one", 1);
        r.close("one");
        assert!(r.commit("one", 1, || panic!("closed window committed")).is_err());
    }

    #[test]
    fn open_requests_do_not_interfere_between_windows() {
        let r = OpenRequests::default();
        r.begin("one", 1);
        r.begin("two", 2);
        assert_eq!(r.commit("one", 1, || 1).unwrap(), 1);
        assert_eq!(r.commit("two", 2, || 2).unwrap(), 2);
    }

    #[test]
    fn delayed_registration_does_not_replace_a_newer_request() {
        let r = OpenRequests::default();
        assert!(r.begin("one", 2));
        assert!(!r.begin("one", 1));
        assert!(r.commit("one", 2, || ()).is_ok());
    }
}
