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

    pub fn byte_len(&self) -> usize {
        with_plane!(self, v => std::mem::size_of_val(v.as_slice()))
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        with_plane!(self, v => {
            let len = std::mem::size_of_val(v.as_slice());
            // SAFETY: every element type here is plain old data with no padding.
            unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, len) }
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
    #[serde(skip)]
    pub samples_per_page: usize,
    #[serde(skip)]
    pub order: DimOrder,
    #[serde(skip)]
    pub page_count: usize,
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
            .thread_name(|i| format!("tifview-bg-{i}"))
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
        // Evict the pages farthest from this one until the new page fits.
        while self.cached_bytes.load(Ordering::Relaxed) + bytes > self.budget && !cache.is_empty() {
            let far = *cache.keys().max_by_key(|&&k| k.abs_diff(page)).unwrap();
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

    /// Decodes pages around `center` in the background so that dragging the Z
    /// slider is instant. Stops when the file is closed or the budget is full.
    pub fn prefetch(self: &Arc<Self>, center: usize) {
        let me = self.clone();
        std::thread::spawn(move || {
            let n = me.info().page_count;
            let per_page = me.info().page_bytes().max(1);
            let max_pages = (me.budget / per_page).max(1).min(n);
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&p| p.abs_diff(center));
            order.truncate(max_pages);
            // Workers pull pages nearest-first from a shared counter.
            let next = AtomicUsize::new(0);
            let pool = bg_pool();
            let workers = pool.current_num_threads().div_ceil(2);
            pool.scope(|s| {
                for _ in 0..workers {
                    s.spawn(|_| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= order.len() || me.cancelled.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = me.page(order[i]);
                    });
                }
            });
        });
    }
}
