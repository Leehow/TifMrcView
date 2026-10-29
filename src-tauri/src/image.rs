//! Format-independent image model: planes, sources, metadata and the
//! per-file decoded-page cache.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rayon::prelude::*;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    F64,
}

impl DType {
    pub fn size(self) -> usize {
        match self {
            DType::U8 | DType::I8 => 1,
            DType::U16 | DType::I16 => 2,
            DType::U32 | DType::I32 | DType::F32 => 4,
            DType::F64 => 8,
        }
    }
}

/// One channel of one 2D image, row-major.
pub enum PlaneData {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

/// Dispatches `$body` with `$v` bound to the typed slice inside a `PlaneData`.
#[macro_export]
macro_rules! with_plane {
    ($data:expr, $v:ident => $body:expr) => {
        match $data {
            $crate::image::PlaneData::U8($v) => $body,
            $crate::image::PlaneData::I8($v) => $body,
            $crate::image::PlaneData::U16($v) => $body,
            $crate::image::PlaneData::I16($v) => $body,
            $crate::image::PlaneData::U32($v) => $body,
            $crate::image::PlaneData::I32($v) => $body,
            $crate::image::PlaneData::F32($v) => $body,
            $crate::image::PlaneData::F64($v) => $body,
        }
    };
}

impl PlaneData {
    pub fn zeroed(dtype: DType, len: usize) -> PlaneData {
        match dtype {
            DType::U8 => PlaneData::U8(vec![0; len]),
            DType::I8 => PlaneData::I8(vec![0; len]),
            DType::U16 => PlaneData::U16(vec![0; len]),
            DType::I16 => PlaneData::I16(vec![0; len]),
            DType::U32 => PlaneData::U32(vec![0; len]),
            DType::I32 => PlaneData::I32(vec![0; len]),
            DType::F32 => PlaneData::F32(vec![0.0; len]),
            DType::F64 => PlaneData::F64(vec![0.0; len]),
        }
    }

    pub fn len(&self) -> usize {
        with_plane!(self, v => v.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn byte_len(&self) -> usize {
        with_plane!(self, v => std::mem::size_of_val(v.as_slice()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        with_plane!(self, v => {
            let len = std::mem::size_of_val(v.as_slice());
            // SAFETY: every element type here is plain old data with no padding.
            unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), len) }
        })
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        with_plane!(self, v => {
            let len = std::mem::size_of_val(v.as_slice());
            // SAFETY: every element type here is plain old data with no padding.
            unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), len) }
        })
    }

    pub fn get_f64(&self, i: usize) -> f64 {
        with_plane!(self, v => v[i].to_f32() as f64)
    }

    /// Splits `channels` interleaved samples into separate planes.
    pub fn deinterleave(self, channels: usize) -> Vec<PlaneData> {
        fn split<T: Copy + Default + Send + Sync>(v: &[T], n: usize) -> Vec<Vec<T>> {
            (0..n)
                .into_par_iter()
                .map(|c| v.iter().skip(c).step_by(n).copied().collect())
                .collect()
        }
        if channels <= 1 {
            return vec![self];
        }
        match self {
            PlaneData::U8(v) => split(&v, channels).into_iter().map(PlaneData::U8).collect(),
            PlaneData::I8(v) => split(&v, channels).into_iter().map(PlaneData::I8).collect(),
            PlaneData::U16(v) => split(&v, channels).into_iter().map(PlaneData::U16).collect(),
            PlaneData::I16(v) => split(&v, channels).into_iter().map(PlaneData::I16).collect(),
            PlaneData::U32(v) => split(&v, channels).into_iter().map(PlaneData::U32).collect(),
            PlaneData::I32(v) => split(&v, channels).into_iter().map(PlaneData::I32).collect(),
            PlaneData::F32(v) => split(&v, channels).into_iter().map(PlaneData::F32).collect(),
            PlaneData::F64(v) => split(&v, channels).into_iter().map(PlaneData::F64).collect(),
        }
    }
}

/// Numeric sample that can be turned into a display value.
pub trait Px: Copy + Send + Sync {
    fn to_f32(self) -> f32;
}
macro_rules! impl_px {
    ($($t:ty),*) => { $(impl Px for $t { #[inline(always)] fn to_f32(self) -> f32 { self as f32 } })* };
}
impl_px!(u8, i8, u16, i16, u32, i32, f32, f64);

pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: PlaneData,
}

/// Summary of a plane used for auto-contrast and slider bounds.
#[derive(Clone)]
pub struct Stats {
    pub min: f32,
    pub max: f32,
    hist: Vec<u32>,
    total: u64,
}

const HIST_BINS: usize = 4096;

impl Stats {
    pub fn compute(p: &Plane) -> Stats {
        // Sample at most ~4M pixels; enough for stable percentiles.
        let n = p.data.len();
        let step = (n / 4_000_000).max(1);
        fn minmax<T: Px>(v: &[T], step: usize) -> (f32, f32) {
            v.par_chunks(65536 * step)
                .map(|c| {
                    let mut lo = f32::INFINITY;
                    let mut hi = f32::NEG_INFINITY;
                    for x in c.iter().step_by(step) {
                        let f = x.to_f32();
                        if f.is_finite() {
                            lo = lo.min(f);
                            hi = hi.max(f);
                        }
                    }
                    (lo, hi)
                })
                .reduce(|| (f32::INFINITY, f32::NEG_INFINITY), |a, b| (a.0.min(b.0), a.1.max(b.1)))
        }
        fn hist<T: Px>(v: &[T], step: usize, lo: f32, hi: f32) -> Vec<u32> {
            let scale = (HIST_BINS as f32 - 1.0) / (hi - lo).max(f32::MIN_POSITIVE);
            v.par_chunks(65536 * step)
                .map(|c| {
                    let mut h = vec![0u32; HIST_BINS];
                    for x in c.iter().step_by(step) {
                        let f = x.to_f32();
                        if f.is_finite() {
                            h[((f - lo) * scale) as usize] += 1;
                        }
                    }
                    h
                })
                .reduce(
                    || vec![0u32; HIST_BINS],
                    |mut a, b| {
                        a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                        a
                    },
                )
        }
        let (mut lo, mut hi) = with_plane!(&p.data, v => minmax(v, step));
        if !lo.is_finite() {
            lo = 0.0;
            hi = 1.0;
        }
        let h = with_plane!(&p.data, v => hist(v, step, lo, hi));
        let total = h.iter().map(|&x| x as u64).sum();
        Stats { min: lo, max: hi, hist: h, total }
    }

    /// Value at the given fraction (0..1) of the cumulative histogram.
    pub fn percentile(&self, frac: f64) -> f32 {
        if self.total == 0 || self.max <= self.min {
            return self.min;
        }
        let target = (self.total as f64 * frac) as u64;
        let mut acc = 0u64;
        for (i, &c) in self.hist.iter().enumerate() {
            acc += c as u64;
            if acc > target {
                return self.min + (self.max - self.min) * (i as f32 / (HIST_BINS as f32 - 1.0));
            }
        }
        self.max
    }

    /// Histogram regrouped into `n` bins between `min` and `max`.
    pub fn bins(&self, n: usize) -> Vec<u32> {
        let mut out = vec![0u32; n];
        for (i, &c) in self.hist.iter().enumerate() {
            out[i * n / HIST_BINS] += c;
        }
        out
    }

    /// ImageJ-like auto contrast: saturate a small fraction at both ends.
    pub fn auto_range(&self) -> (f32, f32) {
        let lo = self.percentile(0.001);
        let mut hi = self.percentile(0.999);
        if hi <= lo {
            hi = self.max.max(lo + 1.0);
        }
        (lo, hi)
    }
}

/// Order of the non-XY dimensions in page storage, fastest first.
#[derive(Clone, Copy, Debug)]
pub enum DimOrder {
    CZT,
    CTZ,
    ZCT,
    ZTC,
    TCZ,
    TZC,
}

impl DimOrder {
    pub fn parse(s: &str) -> DimOrder {
        match s.trim_start_matches("XY") {
            "CTZ" => DimOrder::CTZ,
            "ZCT" => DimOrder::ZCT,
            "ZTC" => DimOrder::ZTC,
            "TCZ" => DimOrder::TCZ,
            "TZC" => DimOrder::TZC,
            _ => DimOrder::CZT,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelInfo {
    pub name: String,
    pub color: [u8; 3],
}

/// A titled list of (label, value) pairs, e.g. one section of SEM settings.
#[derive(Clone, Serialize)]
pub struct MetaGroup {
    pub name: String,
    pub items: Vec<[String; 2]>,
}

/// One image of a file that holds several (a Leica LIF project, ND2 positions…).
#[derive(Clone, Serialize)]
pub struct SeriesInfo {
    pub name: String,
    /// Size and dimensions, for the picker.
    pub detail: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub path: String,
    pub name: String,
    pub format: String,
    pub width: usize,
    pub height: usize,
    pub size_c: usize,
    pub size_z: usize,
    pub size_t: usize,
    pub dtype: DType,
    /// True for 8-bit RGB(A) images that should be shown as-is.
    pub is_rgb: bool,
    pub channels: Vec<ChannelInfo>,
    /// Physical pixel size (x, y, z) and unit, when the file says so.
    pub pixel_size: Option<[f64; 3]>,
    pub unit: String,
    /// Acquisition parameters stored by the instrument, grouped for display.
    pub metadata: Vec<MetaGroup>,
    #[serde(skip)]
    pub samples_per_page: usize,
    #[serde(skip)]
    pub order: DimOrder,
    #[serde(skip)]
    pub page_count: usize,
    /// Every image in the file when it holds several (empty otherwise); the
    /// viewer offers a picker that reopens the file with another index.
    pub series: Vec<SeriesInfo>,
    /// Which entry of `series` is open.
    pub series_index: usize,
    /// For a stack built from the slice images of a folder: that folder.
    pub folder: Option<String>,
}

pub const DEFAULT_COLORS: [[u8; 3]; 7] = [
    [0, 255, 0],
    [255, 0, 255],
    [0, 255, 255],
    [255, 0, 0],
    [0, 0, 255],
    [255, 255, 0],
    [255, 255, 255],
];

impl ImageInfo {
    /// Maps (c, z, t) to (page index, sample index within that page).
    pub fn locate(&self, c: usize, z: usize, t: usize) -> (usize, usize) {
        let (c, z, t) = (
            c.min(self.size_c - 1),
            z.min(self.size_z - 1),
            t.min(self.size_t - 1),
        );
        if self.samples_per_page > 1 {
            return ((t * self.size_z + z).min(self.page_count - 1), c);
        }
        let (sc, sz, st) = (self.size_c, self.size_z, self.size_t);
        let idx = match self.order {
            DimOrder::CZT => c + sc * (z + sz * t),
            DimOrder::CTZ => c + sc * (t + st * z),
            DimOrder::ZCT => z + sz * (c + sc * t),
            DimOrder::ZTC => z + sz * (t + st * c),
            DimOrder::TCZ => t + st * (c + sc * z),
            DimOrder::TZC => t + st * (z + sz * c),
        };
        (idx.min(self.page_count - 1), 0)
    }

    pub fn page_bytes(&self) -> usize {
        self.width * self.height * self.samples_per_page * self.dtype.size()
    }

    pub fn fill_default_channels(&mut self) {
        while self.channels.len() < self.size_c {
            let i = self.channels.len();
            let color = if self.is_rgb {
                [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]][i.min(3)]
            } else if self.size_c == 1 {
                [255, 255, 255]
            } else {
                DEFAULT_COLORS[i % DEFAULT_COLORS.len()]
            };
            self.channels.push(ChannelInfo { name: format!("C{}", i + 1), color });
        }
        self.channels.truncate(self.size_c);
    }
}

/// A decoder for one file. Must be safe to call from several threads at once.
pub trait Source: Send + Sync {
    fn info(&self) -> &ImageInfo;
    /// Decodes one page into one plane per sample.
    fn read_page(&self, page: usize) -> Result<Vec<Plane>>;
    /// The stored bytes of a one-sample page when they sit uncompressed in a
    /// mapped file, and whether they are little-endian. Side views read
    /// single rows from here instead of decoding whole pages.
    fn raw_page(&self, _page: usize) -> Option<(&[u8], bool)> {
        None
    }
    /// For a view built from other pages: (parts done, total) of the page
    /// being built now.
    fn progress(&self) -> Option<(usize, usize)> {
        None
    }
}

pub type Page = Arc<Vec<Plane>>;

/// Pool for background work (prefetch, thumbnails). It leaves one core free
/// so the page the user is looking at always decodes and renders promptly on
/// the global pool.
pub fn bg_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n.saturating_sub(1).max(1))
            .thread_name(|i| format!("tifmrcview-bg-{i}"))
            .build()
            .expect("thread pool")
    })
}

/// An opened file plus its decoded-page cache.
pub struct OpenImage {
    pub token: u64,
    pub source: Box<dyn Source>,
    cache: Mutex<HashMap<usize, Page>>,
    stats: Mutex<HashMap<(usize, usize), Stats>>,
    cached_bytes: AtomicUsize,
    budget: usize,
    pub cancelled: AtomicBool,
    /// The page the user looks at; background reads stay around it.
    focus: AtomicUsize,
}

impl OpenImage {
    pub fn new(token: u64, source: Box<dyn Source>, budget: usize) -> Arc<OpenImage> {
        Arc::new(OpenImage {
            token,
            source,
            cache: Mutex::new(HashMap::new()),
            stats: Mutex::new(HashMap::new()),
            cached_bytes: AtomicUsize::new(0),
            budget,
            cancelled: AtomicBool::new(false),
            focus: AtomicUsize::new(0),
        })
    }

    pub fn info(&self) -> &ImageInfo {
        self.source.info()
    }

    pub fn cached(&self, page: usize) -> Option<Page> {
        self.cache.lock().unwrap().get(&page).cloned()
    }

    pub fn page(&self, page: usize) -> Result<Page> {
        if let Some(p) = self.cached(page) {
            return Ok(p);
        }
        let planes: Page = Arc::new(self.source.read_page(page)?);
        let bytes: usize = planes.iter().map(|p| p.data.byte_len()).sum();
        let mut cache = self.cache.lock().unwrap();
        if let Some(p) = cache.get(&page) {
            return Ok(p.clone());
        }
        // Evict the pages farthest from where the user is until the new page fits.
        let focus = self.focus.load(Ordering::Relaxed);
        while self.cached_bytes.load(Ordering::Relaxed) + bytes > self.budget && !cache.is_empty() {
            let far = *cache.keys().max_by_key(|&&k| k.abs_diff(focus)).unwrap();
            let gone = cache.remove(&far).unwrap();
            let freed: usize = gone.iter().map(|p| p.data.byte_len()).sum();
            self.cached_bytes.fetch_sub(freed, Ordering::Relaxed);
        }
        self.cached_bytes.fetch_add(bytes, Ordering::Relaxed);
        cache.insert(page, planes.clone());
        Ok(planes)
    }

    pub fn stats(&self, page: usize, sample: usize, planes: &Page) -> Stats {
        if let Some(s) = self.stats.lock().unwrap().get(&(page, sample)) {
            return s.clone();
        }
        let s = Stats::compute(&planes[sample.min(planes.len() - 1)]);
        self.stats.lock().unwrap().insert((page, sample), s.clone());
        s
    }

    /// How many pages are in memory, and whether the whole file fits.
    pub fn load_progress(&self) -> (usize, usize, bool) {
        if let Some((done, total)) = self.source.progress() {
            return (done, total, true);
        }
        let n = self.info().page_count;
        let fits = self.info().page_bytes().saturating_mul(n) <= self.budget;
        (self.cache.lock().unwrap().len(), n, fits)
    }

    /// Tells the background reader which page the user is on now.
    pub fn set_focus(&self, page: usize) {
        self.focus.store(page, Ordering::Relaxed);
    }

    /// Decodes pages around the focus page in the background so that dragging
    /// the Z slider is instant. It follows the focus as the user moves, always
    /// reading the nearest page that is not in memory yet, and stops when the
    /// file is closed. Statistics are computed here too, off the UI path.
    pub fn prefetch(self: &Arc<Self>, center: usize) {
        self.set_focus(center);
        let me = self.clone();
        std::thread::spawn(move || {
            let n = me.info().page_count;
            let per_page = me.info().page_bytes().max(1);
            // Pages kept on each side of the focus.
            let reach = (me.budget / per_page).max(1).min(n) / 2;
            let busy = Mutex::new(std::collections::HashSet::new());
            // Pages that failed to decode; never retried in the background.
            let failed = Mutex::new(std::collections::HashSet::new());
            let next = || -> Option<usize> {
                let f = me.focus.load(Ordering::Relaxed).min(n - 1);
                let cache = me.cache.lock().unwrap();
                let mut busy = busy.lock().unwrap();
                let failed = failed.lock().unwrap();
                for d in 0..=reach {
                    for p in [f.checked_add(d), f.checked_sub(d)].into_iter().flatten() {
                        if p < n && !cache.contains_key(&p) && !busy.contains(&p) && !failed.contains(&p) {
                            busy.insert(p);
                            return Some(p);
                        }
                    }
                }
                None
            };
            // Two readers: enough to keep a disk busy without making a slow
            // external drive seek back and forth between far-apart pages.
            // Own threads, so the shared pool stays free for thumbnails.
            std::thread::scope(|s| {
                for _ in 0..2 {
                    s.spawn(|| {
                        while !me.cancelled.load(Ordering::Relaxed) {
                            let Some(p) = next() else {
                                std::thread::sleep(std::time::Duration::from_millis(40));
                                continue;
                            };
                            // A malformed file may panic inside a decoder; treat that as a failed page.
                            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let Ok(planes) = me.page(p) else { return false };
                                for i in 0..planes.len() {
                                    me.stats(p, i, &planes);
                                }
                                true
                            }));
                            if !matches!(ok, Ok(true)) {
                                failed.lock().unwrap().insert(p);
                            }
                            busy.lock().unwrap().remove(&p);
                        }
                    });
                }
            });
        });
    }
}

#[cfg(test)]
mod prefetch_tests {
    use super::*;

    /// 1×1 pages; page 1 fails to decode and page 2 panics.
    struct Flaky {
        info: ImageInfo,
        reads: Arc<[AtomicUsize; 3]>,
    }

    impl Source for Flaky {
        fn info(&self) -> &ImageInfo {
            &self.info
        }
        fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
            self.reads[page].fetch_add(1, Ordering::Relaxed);
            anyhow::ensure!(page != 1, "broken page");
            assert!(page != 2, "decoder bug");
            Ok(vec![Plane { w: 1, h: 1, data: PlaneData::U8(vec![0]) }])
        }
    }

    #[test]
    fn prefetch_skips_failed_pages() {
        let info = ImageInfo {
            path: String::new(),
            name: String::new(),
            format: String::new(),
            width: 1,
            height: 1,
            size_c: 1,
            size_z: 3,
            size_t: 1,
            dtype: DType::U8,
            is_rgb: false,
            channels: vec![],
            pixel_size: None,
            unit: String::new(),
            metadata: vec![],
            samples_per_page: 1,
            order: DimOrder::CZT,
            page_count: 3,
            series: vec![],
            series_index: 0,
            folder: None,
        };
        let reads: Arc<[AtomicUsize; 3]> = Arc::default();
        let img = OpenImage::new(1, Box::new(Flaky { info, reads: reads.clone() }), usize::MAX);
        img.prefetch(1);
        std::thread::sleep(std::time::Duration::from_millis(300));
        img.cancelled.store(true, Ordering::Relaxed);
        assert_eq!(reads[1].load(Ordering::Relaxed), 1);
        assert_eq!(reads[2].load(Ordering::Relaxed), 1);
        assert!(img.cached(0).is_some());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An `ImageInfo` for tests; `page_count` follows from the dimensions.
    pub(crate) fn fake_info(w: usize, h: usize, c: usize, z: usize, t: usize, dtype: DType) -> ImageInfo {
        let mut info = ImageInfo {
            path: String::new(),
            name: "fake".into(),
            format: "fake".into(),
            width: w,
            height: h,
            size_c: c,
            size_z: z,
            size_t: t,
            dtype,
            is_rgb: false,
            channels: vec![],
            pixel_size: None,
            unit: String::new(),
            metadata: vec![],
            samples_per_page: 1,
            order: DimOrder::CZT,
            page_count: c * z * t,
            series: vec![],
            series_index: 0,
            folder: None,
        };
        info.fill_default_channels();
        info
    }

    /// In-memory 8-bit source: `pages[page][sample]` are the row-major bytes.
    pub(crate) struct FakeSource {
        pub info: ImageInfo,
        pub pages: Vec<Vec<Vec<u8>>>,
        pub reads: Arc<AtomicUsize>,
    }

    impl FakeSource {
        pub(crate) fn new(info: ImageInfo, pages: Vec<Vec<Vec<u8>>>) -> (FakeSource, Arc<AtomicUsize>) {
            let reads = Arc::new(AtomicUsize::new(0));
            (FakeSource { info, pages, reads: reads.clone() }, reads)
        }
    }

    impl Source for FakeSource {
        fn info(&self) -> &ImageInfo {
            &self.info
        }
        fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let p = self.pages.get(page).ok_or_else(|| anyhow::anyhow!("no page {page}"))?;
            Ok(p.iter()
                .map(|b| Plane { w: self.info.width, h: self.info.height, data: PlaneData::U8(b.clone()) })
                .collect())
        }
    }

    /// Single-channel stack of `n` pages, each 2x2 (4 bytes) filled with its index.
    fn stack(n: usize, budget: usize) -> (Arc<OpenImage>, Arc<AtomicUsize>) {
        let pages = (0..n).map(|i| vec![vec![i as u8; 4]]).collect();
        let (src, reads) = FakeSource::new(fake_info(2, 2, 1, n, 1, DType::U8), pages);
        (OpenImage::new(1, Box::new(src), budget), reads)
    }

    fn plane(data: PlaneData, w: usize, h: usize) -> Plane {
        Plane { w, h, data }
    }

    #[test]
    fn dtype_sizes() {
        let all = [DType::U8, DType::I8, DType::U16, DType::I16, DType::U32, DType::I32, DType::F32, DType::F64];
        let sizes: Vec<usize> = all.iter().map(|d| d.size()).collect();
        assert_eq!(sizes, [1, 1, 2, 2, 4, 4, 4, 8]);
    }

    #[test]
    fn plane_data_basics() {
        let p = PlaneData::zeroed(DType::U16, 3);
        assert_eq!((p.len(), p.byte_len()), (3, 6));
        let p = PlaneData::U16(vec![1, 0x0203]);
        let mut want = 1u16.to_ne_bytes().to_vec();
        want.extend(0x0203u16.to_ne_bytes());
        assert_eq!(p.as_bytes(), want);
        assert_eq!(PlaneData::I16(vec![-5]).get_f64(0), -5.0);
        let mut p = PlaneData::zeroed(DType::U8, 2);
        p.as_bytes_mut()[1] = 9;
        assert_eq!(p.get_f64(1), 9.0);
    }

    #[test]
    fn deinterleave_splits_samples() {
        let planes = PlaneData::U8(vec![1, 10, 100, 2, 20, 200]).deinterleave(3);
        let got: Vec<Vec<u8>> = planes
            .iter()
            .map(|p| match p {
                PlaneData::U8(v) => v.clone(),
                _ => panic!("dtype changed"),
            })
            .collect();
        assert_eq!(got, [vec![1, 2], vec![10, 20], vec![100, 200]]);
    }

    #[test]
    fn deinterleave_single_channel_is_identity() {
        let planes = PlaneData::F32(vec![1.0, 2.0, 3.0]).deinterleave(1);
        assert_eq!(planes.len(), 1);
        assert_eq!(planes[0].len(), 3);
        assert_eq!(PlaneData::F32(vec![1.0]).deinterleave(0).len(), 1);
    }

    #[test]
    fn deinterleave_f32_pairs() {
        let planes = PlaneData::F32(vec![0.5, -0.5, 1.5, -1.5]).deinterleave(2);
        assert_eq!((planes[0].get_f64(1), planes[1].get_f64(1)), (1.5, -1.5));
    }

    #[test]
    fn stats_constant_image() {
        let s = Stats::compute(&plane(PlaneData::U8(vec![7; 16]), 4, 4));
        assert_eq!((s.min, s.max), (7.0, 7.0));
        assert_eq!(s.percentile(0.5), 7.0);
        let (lo, hi) = s.auto_range();
        assert_eq!(lo, 7.0);
        assert!(hi > lo);
        assert_eq!(s.bins(8).iter().sum::<u32>(), 16);
    }

    #[test]
    fn stats_ramp() {
        let s = Stats::compute(&plane(PlaneData::U8((0..=255).collect()), 16, 16));
        assert_eq!((s.min, s.max), (0.0, 255.0));
        assert!((s.percentile(0.5) - 128.0).abs() <= 2.0, "{}", s.percentile(0.5));
        assert!(s.percentile(0.0) <= 1.0);
        assert!(s.percentile(1.0) >= 254.0);
        let (lo, hi) = s.auto_range();
        assert!(lo <= 1.0 && (253.0..=255.0).contains(&hi), "{lo} {hi}");
    }

    #[test]
    fn stats_bins_regroup() {
        let s = Stats::compute(&plane(PlaneData::U8((0..=255).collect()), 16, 16));
        let b = s.bins(4);
        assert_eq!(b.len(), 4);
        assert_eq!(b.iter().sum::<u32>(), 256);
        assert!(b.iter().all(|&c| (60..=68).contains(&c)), "{b:?}");
        assert_eq!(s.bins(1), vec![256]);
    }

    #[test]
    fn stats_f32_ignores_nan_and_inf() {
        let v = vec![f32::NAN, 1.0, f32::INFINITY, 3.0, f32::NEG_INFINITY, 2.0];
        let s = Stats::compute(&plane(PlaneData::F32(v), 3, 2));
        assert_eq!((s.min, s.max), (1.0, 3.0));
        assert_eq!(s.bins(4).iter().sum::<u32>(), 3);
    }

    #[test]
    fn stats_all_nan_falls_back_to_unit_range() {
        let s = Stats::compute(&plane(PlaneData::F32(vec![f32::NAN; 4]), 2, 2));
        assert_eq!((s.min, s.max), (0.0, 1.0));
        assert_eq!(s.percentile(0.5), 0.0);
    }

    #[test]
    fn stats_signed_types() {
        let s = Stats::compute(&plane(PlaneData::I16(vec![-100, 50, 0, 300]), 2, 2));
        assert_eq!((s.min, s.max), (-100.0, 300.0));
        let s = Stats::compute(&plane(PlaneData::I8(vec![-128, 127]), 2, 1));
        assert_eq!((s.min, s.max), (-128.0, 127.0));
    }

    #[test]
    fn dim_order_parse() {
        let name = |s: &str| format!("{:?}", DimOrder::parse(s));
        assert_eq!(name("XYCZT"), "CZT");
        assert_eq!(name("XYCTZ"), "CTZ");
        assert_eq!(name("XYZCT"), "ZCT");
        assert_eq!(name("XYZTC"), "ZTC");
        assert_eq!(name("XYTCZ"), "TCZ");
        assert_eq!(name("XYTZC"), "TZC");
        assert_eq!(name("TZC"), "TZC");
        assert_eq!(name("bogus"), "CZT");
        assert_eq!(name(""), "CZT");
    }

    const ORDERS: [DimOrder; 6] =
        [DimOrder::CZT, DimOrder::CTZ, DimOrder::ZCT, DimOrder::ZTC, DimOrder::TCZ, DimOrder::TZC];

    #[test]
    fn locate_every_order_is_a_bijection() {
        for order in ORDERS {
            let mut info = fake_info(1, 1, 2, 3, 4, DType::U8);
            info.order = order;
            let mut seen = vec![false; info.page_count];
            for t in 0..4 {
                for z in 0..3 {
                    for c in 0..2 {
                        let (p, s) = info.locate(c, z, t);
                        assert_eq!(s, 0);
                        assert!(!seen[p], "{order:?}: page {p} hit twice");
                        seen[p] = true;
                    }
                }
            }
            assert!(seen.iter().all(|&b| b), "{order:?}");
        }
    }

    #[test]
    fn locate_known_indices() {
        let loc = |order, c, z, t| {
            let mut info = fake_info(1, 1, 2, 3, 4, DType::U8);
            info.order = order;
            info.locate(c, z, t).0
        };
        // (c, z, t) = (1, 2, 3); sizes c=2, z=3, t=4.
        assert_eq!(loc(DimOrder::CZT, 1, 2, 3), 1 + 2 * (2 + 3 * 3));
        assert_eq!(loc(DimOrder::CTZ, 1, 2, 3), 1 + 2 * (3 + 4 * 2));
        assert_eq!(loc(DimOrder::ZCT, 1, 2, 3), 2 + 3 * (1 + 2 * 3));
        assert_eq!(loc(DimOrder::ZTC, 1, 2, 3), 2 + 3 * (3 + 4));
        assert_eq!(loc(DimOrder::TCZ, 1, 2, 3), 3 + 4 * (1 + 2 * 2));
        assert_eq!(loc(DimOrder::TZC, 1, 2, 3), 3 + 4 * (2 + 3));
        assert_eq!(loc(DimOrder::CZT, 0, 0, 0), 0);
        assert_eq!(loc(DimOrder::CZT, 1, 0, 0), 1);
        assert_eq!(loc(DimOrder::ZCT, 0, 1, 0), 1);
    }

    #[test]
    fn locate_clamps_out_of_range() {
        let info = fake_info(1, 1, 2, 3, 4, DType::U8);
        assert_eq!(info.locate(99, 99, 99), info.locate(1, 2, 3));
        assert_eq!(info.locate(99, 99, 99).0, info.page_count - 1);
    }

    #[test]
    fn locate_interleaved_samples() {
        // RGB pages: channel is the sample index, pages are (t, z).
        let mut info = fake_info(1, 1, 3, 2, 2, DType::U8);
        info.samples_per_page = 3;
        info.page_count = 4;
        assert_eq!(info.locate(2, 1, 1), (3, 2));
        assert_eq!(info.locate(0, 1, 0), (1, 0));
        assert_eq!(info.locate(7, 7, 7), (3, 2));
    }

    #[test]
    fn locate_clamps_to_page_count() {
        // Metadata claims more planes than the file has.
        let mut info = fake_info(1, 1, 1, 5, 1, DType::U8);
        info.page_count = 3;
        assert_eq!(info.locate(0, 4, 0).0, 2);
    }

    #[test]
    fn page_bytes_counts_samples_and_dtype() {
        let mut info = fake_info(4, 3, 3, 1, 1, DType::U16);
        info.samples_per_page = 3;
        assert_eq!(info.page_bytes(), 4 * 3 * 3 * 2);
    }

    #[test]
    fn default_channels_gray_multi_and_rgb() {
        let info = fake_info(1, 1, 1, 1, 1, DType::U8);
        assert_eq!(info.channels.len(), 1);
        assert_eq!(info.channels[0].color, [255, 255, 255]);
        assert_eq!(info.channels[0].name, "C1");

        let info = fake_info(1, 1, 3, 1, 1, DType::U8);
        let colors: Vec<_> = info.channels.iter().map(|c| c.color).collect();
        assert_eq!(colors, [DEFAULT_COLORS[0], DEFAULT_COLORS[1], DEFAULT_COLORS[2]]);

        let mut rgb = fake_info(1, 1, 3, 1, 1, DType::U8);
        rgb.is_rgb = true;
        rgb.channels.clear();
        rgb.fill_default_channels();
        let colors: Vec<_> = rgb.channels.iter().map(|c| c.color).collect();
        assert_eq!(colors, [[255, 0, 0], [0, 255, 0], [0, 0, 255]]);
    }

    #[test]
    fn default_channels_wrap_and_truncate() {
        let mut info = fake_info(1, 1, 9, 1, 1, DType::U8);
        assert_eq!(info.channels.len(), 9);
        assert_eq!(info.channels[7].color, DEFAULT_COLORS[0]);
        info.size_c = 2;
        info.fill_default_channels();
        assert_eq!(info.channels.len(), 2);
        // Existing entries are kept.
        info.channels[0].name = "DAPI".into();
        info.size_c = 3;
        info.fill_default_channels();
        assert_eq!((info.channels[0].name.as_str(), info.channels[2].name.as_str()), ("DAPI", "C3"));
    }

    #[test]
    fn page_cache_hits_do_not_reread() {
        let (img, reads) = stack(3, 1000);
        assert!(img.cached(0).is_none());
        img.page(0).unwrap();
        img.page(0).unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert!(img.cached(0).is_some());
        assert!(img.page(9).is_err());
    }

    #[test]
    fn page_cache_evicts_farthest_from_focus() {
        // Budget holds two 4-byte pages.
        let (img, reads) = stack(10, 8);
        img.page(0).unwrap();
        img.page(1).unwrap();
        img.set_focus(1);
        img.page(5).unwrap();
        assert!(img.cached(0).is_none(), "page 0 is farthest from focus 1");
        assert!(img.cached(1).is_some() && img.cached(5).is_some());
        // Move focus to the far end: page 1 is now the farthest.
        img.set_focus(9);
        img.page(8).unwrap();
        assert!(img.cached(1).is_none());
        assert!(img.cached(5).is_some() && img.cached(8).is_some());
        assert_eq!(reads.load(Ordering::Relaxed), 4);
        // Evicted pages are decoded again.
        img.page(0).unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn page_larger_than_budget_is_still_returned() {
        let (img, _) = stack(3, 2);
        let p = img.page(1).unwrap();
        assert_eq!(p[0].data.get_f64(0), 1.0);
        img.page(2).unwrap();
        assert!(img.cached(1).is_none() && img.cached(2).is_some());
    }

    #[test]
    fn stats_are_cached_per_page_and_sample() {
        let (img, _) = stack(2, 1000);
        let p = img.page(1).unwrap();
        let s = img.stats(1, 0, &p);
        assert_eq!((s.min, s.max), (1.0, 1.0));
        // Out-of-range sample index is clamped.
        assert_eq!(img.stats(1, 5, &p).min, 1.0);
    }
}
