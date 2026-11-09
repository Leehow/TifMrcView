//! Source handles owned by Pro tabs and merge documents in one window.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::image::OpenImage;

#[derive(Default)]
pub(crate) struct Documents(Mutex<HashMap<String, HashMap<u64, Arc<OpenImage>>>>);

impl Documents {
    pub fn keep(&self, window: &str, image: Arc<OpenImage>) {
        self.0.lock().unwrap().entry(window.into()).or_default().insert(image.token, image);
    }

    pub fn get(&self, window: &str, token: u64) -> Option<Arc<OpenImage>> {
        self.0.lock().unwrap().get(window)?.get(&token).cloned()
    }

    pub fn find(&self, token: u64) -> Option<Arc<OpenImage>> {
        self.0.lock().unwrap().values().find_map(|images| images.get(&token).cloned())
    }

    pub fn all(&self) -> Vec<Arc<OpenImage>> {
        self.0.lock().unwrap().values().flat_map(|images| images.values().cloned()).collect()
    }

    pub fn remove(&self, window: &str, token: u64) {
        if let Some(image) = self.0.lock().unwrap().get_mut(window).and_then(|images| images.remove(&token)) {
            image.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn close_window(&self, window: &str) {
        if let Some(images) = self.0.lock().unwrap().remove(window) {
            for image in images.values() {
                image.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    pub fn close_all(&self) {
        let windows = std::mem::take(&mut *self.0.lock().unwrap());
        for images in windows.values() {
            for image in images.values() { image.cancelled.store(true, std::sync::atomic::Ordering::Relaxed); }
        }
    }

    pub fn rebalance(&self, budget: usize) {
        let images = self.all();
        let share = budget / images.len().max(1);
        for image in images { image.set_cache_budget(share); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{tests::{fake_info, FakeSource}, DType};
    use std::sync::atomic::Ordering;

    fn source(token: u64) -> Arc<OpenImage> {
        let (source, _) = FakeSource::new(fake_info(2, 2, 1, 4, 1, DType::U8), vec![vec![vec![7; 4]]; 4]);
        OpenImage::new(token, Box::new(source), 1024)
    }

    #[test]
    fn windows_retain_independent_sources_and_close_only_their_own() {
        let docs = Documents::default();
        let a = source(1); let b = source(2);
        docs.keep("one", a.clone()); docs.keep("two", b.clone());
        assert!(docs.get("two", 1).is_none());
        assert_eq!(docs.find(1).unwrap().token, 1);
        docs.remove("two", 1);
        assert!(!a.cancelled.load(Ordering::Relaxed));
        docs.close_window("one");
        assert!(a.cancelled.load(Ordering::Relaxed));
        assert!(!b.cancelled.load(Ordering::Relaxed));
        assert!(docs.find(1).is_none());
        assert_eq!(docs.all().len(), 1);
        docs.close_all();
        assert!(b.cancelled.load(Ordering::Relaxed));
        assert!(docs.all().is_empty());
    }

    #[test]
    fn adding_tabs_rebalances_and_evicts_existing_decoded_pages() {
        let docs = Documents::default();
        let a = source(1); let b = source(2);
        for image in [&a, &b] { for page in 0..4 { image.page(page).unwrap(); } }
        docs.keep("one", a.clone()); docs.keep("one", b.clone());
        docs.rebalance(8);
        for image in [&a, &b] {
            let retained = (0..4).filter(|&page| image.cached(page).is_some()).count();
            assert_eq!(retained, 1);
            assert!(!image.load_progress().2);
            // An uncached plane remains readable after eviction.
            assert_eq!(image.page(3).unwrap()[0].data.len(), 4);
        }
    }
}
