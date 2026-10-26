//! SDK for TifMrcView format and metadata plugins (WebAssembly, ABI v1).
//!
//! A plugin is a `cdylib` crate built for `wasm32-unknown-unknown`. It
//! implements [`Format`] (to open a new file type) and/or [`Metadata`] (to
//! add settings to files the core opens) and exports them with
//! [`export_format!`] / [`export_metadata!`]. The ABI itself is described in
//! `docs/format-plugins.md`; this crate only wraps it.
//!
//! The plugin never sees the file system. It reads the one file it was
//! asked about through [`file`], and describes each page as a [`Recipe`]:
//! a list of byte ranges of the file plus how to decompress them and where
//! to put the pixels. The host then does the heavy lifting natively and in
//! parallel, so a plugin is as fast as built-in code.
//!
//! ```ignore
//! use tifmrcview_plugin_sdk::*;
//!
//! struct Raw { w: u32, h: u32 }
//!
//! impl Format for Raw {
//!     fn open(_path: &str) -> Result<Raw> {
//!         let head = file::read(0, 8)?;
//!         Ok(Raw { w: le_u32(&head, 0), h: le_u32(&head, 4) })
//!     }
//!     fn info(&mut self) -> Result<Info> {
//!         Ok(Info::new(self.w, self.h, DType::U16, "Raw"))
//!     }
//!     fn page(&self, _page: u32) -> Result<Recipe> {
//!         let mut r = Recipe::new();
//!         r.push(Chunk::file(8, self.w as u64 * self.h as u64 * 2, [0, 0, self.w, self.h]));
//!         Ok(r)
//!     }
//! }
//! export_format!(Raw);
//! ```

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::fmt::Write as _;

pub use alloc::{format, vec};

/// The ABI version this SDK implements (`tmv_abi`).
pub const ABI_VERSION: u32 = 1;

// ------------------------------------------------------------------ errors

/// An error message shown to the user (in the app's language: Chinese).
#[derive(Debug, Clone)]
pub struct Error(pub String);

impl From<&str> for Error {
    fn from(s: &str) -> Error {
        Error(s.to_string())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Error {
        Error(s)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Returns early with an error built like `format!`.
#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::Error($crate::format!($($t)*))) };
}

/// `Option` → [`Result`] with a message.
pub trait Context<T> {
    fn context(self, msg: &str) -> Result<T>;
}

impl<T> Context<T> for Option<T> {
    fn context(self, msg: &str) -> Result<T> {
        self.ok_or_else(|| Error(msg.to_string()))
    }
}

// ------------------------------------------------------------------ host imports

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "tmv")]
    extern "C" {
        pub fn file_size() -> u64;
        pub fn read(offset: u64, len: u32, dst: *mut u8) -> i32;
        pub fn decompress(codec: u32, src: *const u8, src_len: u32, dst: *mut u8, dst_len: u32) -> i32;
        pub fn log(ptr: *const u8, len: u32);
        pub fn error(ptr: *const u8, len: u32);
    }
}

/// Stand-ins so the crate also builds (and type-checks) for the host.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::missing_safety_doc)]
mod imports {
    pub unsafe fn file_size() -> u64 {
        0
    }
    pub unsafe fn read(_offset: u64, _len: u32, _dst: *mut u8) -> i32 {
        -1
    }
    pub unsafe fn decompress(_codec: u32, _src: *const u8, _src_len: u32, _dst: *mut u8, _dst_len: u32) -> i32 {
        -1
    }
    pub unsafe fn log(_ptr: *const u8, _len: u32) {}
    pub unsafe fn error(_ptr: *const u8, _len: u32) {}
}

/// The file the host asked about. Reads are bounds-checked by the host.
pub mod file {
    use super::*;

    /// Size of the file in bytes.
    pub fn size() -> u64 {
        // SAFETY: plain host call.
        unsafe { imports::file_size() }
    }

    /// Reads into `buf`; returns how many bytes were read (fewer at the end of the file).
    pub fn read_into(offset: u64, buf: &mut [u8]) -> Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let n = (buf.len() - done).min(1 << 30) as u32;
            // SAFETY: `buf[done..]` is valid for `n` bytes.
            let r = unsafe { imports::read(offset + done as u64, n, buf[done..].as_mut_ptr()) };
            if r < 0 {
                bail!("读取文件失败");
            }
            done += r as usize;
            if (r as u32) < n {
                break;
            }
        }
        Ok(done)
    }

    /// Reads exactly `len` bytes at `offset`, or fails ("file truncated").
    pub fn read(offset: u64, len: usize) -> Result<Vec<u8>> {
        if offset.checked_add(len as u64).is_none_or(|end| end > size()) {
            bail!("文件被截断（需要读取 {len} 字节，位置 {offset}）");
        }
        let mut v = vec![0u8; len];
        read_into(offset, &mut v)?;
        Ok(v)
    }

    /// Reads up to `len` bytes at `offset`.
    pub fn read_at_most(offset: u64, len: usize) -> Result<Vec<u8>> {
        let len = len.min(size().saturating_sub(offset) as usize);
        let mut v = vec![0u8; len];
        let n = read_into(offset, &mut v)?;
        v.truncate(n);
        Ok(v)
    }
}

/// Decompresses a small block (e.g. metadata) natively. `max_out` caps the output.
pub fn decompress(codec: Codec, src: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; max_out];
    // SAFETY: both buffers are valid for the given lengths.
    let n = unsafe { imports::decompress(codec as u32, src.as_ptr(), src.len() as u32, out.as_mut_ptr(), max_out as u32) };
    if n < 0 {
        bail!("解压失败");
    }
    out.truncate(n as usize);
    Ok(out)
}

/// Writes a line to the app's log (stderr), for debugging.
pub fn log(msg: &str) {
    // SAFETY: plain host call with a valid slice.
    unsafe { imports::log(msg.as_ptr(), msg.len() as u32) }
}

#[doc(hidden)]
pub fn set_error(msg: &str) {
    // SAFETY: plain host call with a valid slice.
    unsafe { imports::error(msg.as_ptr(), msg.len() as u32) }
}

// ------------------------------------------------------------------ little helpers

pub fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub fn le_i32(b: &[u8], o: usize) -> i32 {
    le_u32(b, o) as i32
}
pub fn le_u64(b: &[u8], o: usize) -> u64 {
    le_u32(b, o) as u64 | (le_u32(b, o + 4) as u64) << 32
}
pub fn le_i64(b: &[u8], o: usize) -> i64 {
    le_u64(b, o) as i64
}

// ------------------------------------------------------------------ types

/// Sample type. The numbers are the ABI codes used in recipes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DType {
    U8 = 0,
    I8 = 1,
    U16 = 2,
    I16 = 3,
    U32 = 4,
    I32 = 5,
    F32 = 6,
    F64 = 7,
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
    pub fn name(self) -> &'static str {
        ["u8", "i8", "u16", "i16", "u32", "i32", "f32", "f64"][self as usize]
    }
}

/// Compression of a chunk (also for [`decompress`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    Raw = 0,
    /// zlib stream (RFC 1950), as in TIFF compression 8.
    Zlib = 1,
    /// Raw deflate (RFC 1951).
    Deflate = 2,
    /// TIFF-style LZW (MSB first, early change).
    Lzw = 3,
    Zstd = 4,
    /// Baseline JPEG; 8-bit gray (1 sample) or RGB (3 samples).
    Jpeg = 5,
    /// PackBits (TIFF compression 32773).
    PackBits = 6,
}

/// Dimension order of pages, fastest first (like OME's `DimensionOrder` without XY).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    CZT,
    CTZ,
    ZCT,
    ZTC,
    TCZ,
    TZC,
}

impl Order {
    fn name(self) -> &'static str {
        match self {
            Order::CZT => "CZT",
            Order::CTZ => "CTZ",
            Order::ZCT => "ZCT",
            Order::ZTC => "ZTC",
            Order::TCZ => "TCZ",
            Order::TZC => "TZC",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub name: String,
    pub color: [u8; 3],
}

/// A titled list of (label, value) pairs shown in the image-info panel.
#[derive(Clone, Debug, Default)]
///
/// Items are written straight into their JSON form: a group with tens of
/// thousands of entries costs one growing string, not two allocations per
/// item (which matters in an interpreter).
pub struct Group {
    pub name: String,
    /// `["label","value"],…` without the brackets around the list.
    json: String,
    len: usize,
}

impl Group {
    pub fn new(name: &str) -> Group {
        Group { name: name.to_string(), json: String::new(), len: 0 }
    }
    /// Adds an item unless the value is missing or empty.
    pub fn add(&mut self, label: &str, value: Option<String>) {
        if let Some(v) = value {
            self.add_str(label, &v);
        }
    }
    /// Adds an item unless the value is empty.
    pub fn add_str(&mut self, label: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        if self.len > 0 {
            self.json.push(',');
        }
        self.json.push('[');
        json_str(&mut self.json, label);
        self.json.push(',');
        json_str(&mut self.json, value);
        self.json.push(']');
        self.len += 1;
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Appends the group to `out` if it has any items.
    pub fn push_to(self, out: &mut Vec<Group>) {
        if !self.is_empty() {
            out.push(self);
        }
    }
}

/// What a format plugin says about an open file (`tmv_info`).
#[derive(Clone, Debug)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub size_c: u32,
    pub size_z: u32,
    pub size_t: u32,
    pub dtype: DType,
    /// 8-bit RGB shown as-is instead of as channels.
    pub is_rgb: bool,
    /// Planes in each page: 1, or `size_c` when every page holds all channels.
    pub samples_per_page: u32,
    pub order: Order,
    pub page_count: u32,
    /// Fewer than `size_c` entries are filled with defaults.
    pub channels: Vec<Channel>,
    /// (x, y, z) in `unit`.
    pub pixel_size: Option<[f64; 3]>,
    /// "Å", "nm", "µm" or "mm".
    pub unit: String,
    /// Shown in the status bar, e.g. "Zeiss CZI".
    pub format: String,
    pub metadata: Vec<Group>,
}

impl Info {
    /// A single 2D image; adjust the fields for stacks.
    pub fn new(width: u32, height: u32, dtype: DType, format: &str) -> Info {
        Info {
            width,
            height,
            size_c: 1,
            size_z: 1,
            size_t: 1,
            dtype,
            is_rgb: false,
            samples_per_page: 1,
            order: Order::CZT,
            page_count: 1,
            channels: vec![],
            pixel_size: None,
            unit: String::new(),
            format: format.to_string(),
            metadata: vec![],
        }
    }

    pub fn to_json(&self) -> String {
        let mut j = Json::object();
        j.num("width", self.width as f64);
        j.num("height", self.height as f64);
        j.num("sizeC", self.size_c as f64);
        j.num("sizeZ", self.size_z as f64);
        j.num("sizeT", self.size_t as f64);
        j.str("dtype", self.dtype.name());
        j.bool("isRgb", self.is_rgb);
        j.num("samplesPerPage", self.samples_per_page as f64);
        j.str("order", self.order.name());
        j.num("pageCount", self.page_count as f64);
        j.raw("channels", &channels_json(&self.channels));
        j.raw("pixelSize", &pixel_size_json(self.pixel_size));
        j.str("unit", &self.unit);
        j.str("format", &self.format);
        j.raw("metadata", &groups_json(&self.metadata));
        j.finish()
    }
}

/// What a metadata plugin adds to a file the core opened (`tmv_metadata`).
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// Replaces the format name when set.
    pub format: Option<String>,
    /// Replaces the pixel size (x, y, z; z may be 0) when set; needs `unit`.
    pub pixel_size: Option<[f64; 3]>,
    pub unit: Option<String>,
    /// Appended to the image-info panel.
    pub groups: Vec<Group>,
}

impl Meta {
    pub fn to_json(&self) -> String {
        let mut j = Json::object();
        if let Some(f) = &self.format {
            j.str("format", f);
        }
        if self.pixel_size.is_some() {
            j.raw("pixelSize", &pixel_size_json(self.pixel_size));
        }
        if let Some(u) = &self.unit {
            j.str("unit", u);
        }
        j.raw("groups", &groups_json(&self.groups));
        j.finish()
    }
}

fn channels_json(ch: &[Channel]) -> String {
    let mut s = String::from("[");
    for (i, c) in ch.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let mut o = Json::object();
        o.str("name", &c.name);
        o.raw("color", &format!("[{},{},{}]", c.color[0], c.color[1], c.color[2]));
        s.push_str(&o.finish());
    }
    s.push(']');
    s
}

fn pixel_size_json(ps: Option<[f64; 3]>) -> String {
    match ps {
        Some([x, y, z]) => format!("[{},{},{}]", num(x), num(y), num(z)),
        None => "null".into(),
    }
}

fn groups_json(groups: &[Group]) -> String {
    let mut s = String::from("[");
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"name\":");
        json_str(&mut s, &g.name);
        s.push_str(",\"items\":[");
        s.push_str(&g.json);
        s.push_str("]}");
    }
    s.push(']');
    s
}

// ------------------------------------------------------------------ JSON

/// Appends `v` as a JSON string literal.
pub fn json_str(out: &mut String, v: &str) {
    out.push('"');
    // Copy runs of plain characters in one go; only escapes are handled one by one.
    let b = v.as_bytes();
    let mut run = 0;
    while let Some(k) = b[run..].iter().position(|&c| c < 0x20 || c == b'"' || c == b'\\') {
        let i = run + k;
        out.push_str(&v[run..i]);
        match b[i] {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            c => {
                let _ = write!(out, "\\u{:04x}", c);
            }
        }
        run = i + 1;
    }
    out.push_str(&v[run..]);
    out.push('"');
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".into()
    }
}

/// A tiny JSON object writer (no serde, to keep plugins small).
pub struct Json {
    s: String,
    first: bool,
}

impl Json {
    pub fn object() -> Json {
        Json { s: String::from("{"), first: true }
    }
    fn key(&mut self, k: &str) {
        if !self.first {
            self.s.push(',');
        }
        self.first = false;
        json_str(&mut self.s, k);
        self.s.push(':');
    }
    pub fn str(&mut self, k: &str, v: &str) {
        self.key(k);
        json_str(&mut self.s, v);
    }
    pub fn num(&mut self, k: &str, v: f64) {
        self.key(k);
        self.s.push_str(&num(v));
    }
    pub fn bool(&mut self, k: &str, v: bool) {
        self.key(k);
        self.s.push_str(if v { "true" } else { "false" });
    }
    /// `v` must already be valid JSON.
    pub fn raw(&mut self, k: &str, v: &str) {
        self.key(k);
        self.s.push_str(v);
    }
    pub fn finish(mut self) -> String {
        self.s.push('}');
        self.s
    }
}

// ------------------------------------------------------------------ recipes

/// Chunk flags.
pub mod flags {
    /// Samples are big-endian.
    pub const BIG_ENDIAN: u8 = 1;
    /// 16-bit samples stored as all low bytes, then all high bytes (CZI zstd "hi/lo").
    pub const HILO: u8 = 2;
    /// The first three samples are B, G, R: sample 0 goes to plane `plane + 2`, sample 2 to `plane`.
    pub const BGR: u8 = 4;
    /// TIFF horizontal differencing (predictor 2), undone per row after decoding (integer types).
    pub const PREDICTOR: u8 = 8;
}

/// One block of pixels: where its bytes are, how they are compressed and
/// which rectangle of the page they fill.
#[derive(Clone, Debug)]
pub struct Chunk {
    /// 0: `offset` is a file offset; 1: an offset into the recipe's inline payload.
    pub source: u8,
    pub codec: Codec,
    /// `None` = the page's dtype.
    pub dtype: Option<DType>,
    pub flags: u8,
    /// Interleaved samples per pixel in the decoded data.
    pub samples: u32,
    pub offset: u64,
    pub length: u64,
    /// Destination rectangle (x, y, w, h) in page pixels; clipped to the page.
    pub rect: [u32; 4],
    /// Destination plane of sample 0; samples go to consecutive planes.
    /// Samples landing on planes the page does not have are dropped.
    pub plane: u32,
    /// Bytes per decoded row; 0 = `w * samples * dtype size`.
    pub stride: u32,
}

impl Chunk {
    /// Uncompressed single-sample bytes at `offset..offset+length` of the file.
    pub fn file(offset: u64, length: u64, rect: [u32; 4]) -> Chunk {
        Chunk { source: 0, codec: Codec::Raw, dtype: None, flags: 0, samples: 1, offset, length, rect, plane: 0, stride: 0 }
    }
    pub fn codec(mut self, c: Codec) -> Chunk {
        self.codec = c;
        self
    }
    pub fn samples(mut self, n: u32) -> Chunk {
        self.samples = n;
        self
    }
    pub fn plane(mut self, p: u32) -> Chunk {
        self.plane = p;
        self
    }
    pub fn flags(mut self, f: u8) -> Chunk {
        self.flags |= f;
        self
    }
    pub fn stride(mut self, s: u32) -> Chunk {
        self.stride = s;
        self
    }
}

/// How to build one page. See [`Chunk`]. Chunks are applied in order, so
/// later chunks are drawn over earlier ones where they overlap.
#[derive(Clone, Debug, Default)]
pub struct Recipe {
    pub chunks: Vec<Chunk>,
    payload: Vec<u8>,
}

pub const RECIPE_MAGIC: &[u8; 4] = b"TMVR";
pub const CHUNK_SIZE: usize = 48;
/// Chunk dtype code meaning "the page's dtype".
pub const DTYPE_OF_PAGE: u8 = 0xff;

impl Recipe {
    pub fn new() -> Recipe {
        Recipe::default()
    }

    pub fn push(&mut self, c: Chunk) {
        self.chunks.push(c);
    }

    /// Adds bytes the plugin produced itself (decoded or still compressed
    /// with `codec`), placed at `rect`. Returns the chunk for further tweaks.
    pub fn inline(&mut self, bytes: &[u8], codec: Codec, rect: [u32; 4]) -> &mut Chunk {
        let offset = self.payload.len() as u64;
        self.payload.extend_from_slice(bytes);
        self.chunks.push(Chunk { source: 1, codec, dtype: None, flags: 0, samples: 1, offset, length: bytes.len() as u64, rect, plane: 0, stride: 0 });
        self.chunks.last_mut().unwrap()
    }

    /// The binary form `tmv_page` returns.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.chunks.len() * CHUNK_SIZE + self.payload.len());
        out.extend_from_slice(RECIPE_MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for c in &self.chunks {
            out.push(c.source);
            out.push(c.codec as u8);
            out.push(c.dtype.map_or(DTYPE_OF_PAGE, |d| d as u8));
            out.push(c.flags);
            out.extend_from_slice(&c.samples.to_le_bytes());
            out.extend_from_slice(&c.offset.to_le_bytes());
            out.extend_from_slice(&c.length.to_le_bytes());
            for v in c.rect {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&c.plane.to_le_bytes());
            out.extend_from_slice(&c.stride.to_le_bytes());
        }
        out.extend_from_slice(&self.payload);
        out
    }
}

// ------------------------------------------------------------------ plugin traits

/// A file format. One value per open file.
pub trait Format: Sized + 'static {
    /// Inspects the file (through [`file`]) and builds whatever index the
    /// pages need. `path` is for information only (name, extension).
    fn open(path: &str) -> Result<Self>;
    /// Called once, right after `open`; may move data out of `self`.
    fn info(&mut self) -> Result<Info>;
    /// How to build page `page` (0-based; the order given by [`Info::order`]).
    fn page(&self, page: u32) -> Result<Recipe>;
}

/// Extra metadata for files the core (or another plugin) opens.
pub trait Metadata {
    /// `Ok(None)` when the file is not one this plugin knows.
    fn metadata(path: &str) -> Result<Option<Meta>>;
}

// ------------------------------------------------------------------ runtime glue

/// Global state for a single-threaded wasm instance.
#[doc(hidden)]
pub struct Global<T>(UnsafeCell<T>);

// SAFETY: wasm32-unknown-unknown plugins are single-threaded; the host
// never calls into one instance from two threads at once.
unsafe impl<T> Sync for Global<T> {}

impl<T> Global<T> {
    pub const fn new(v: T) -> Global<T> {
        Global(UnsafeCell::new(v))
    }
    /// # Safety
    /// No other reference from `get` may be alive.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self) -> &mut T {
        &mut *self.0.get()
    }
}

static RESULT: Global<Vec<u8>> = Global::new(Vec::new());

/// Keeps `bytes` alive until the next call and returns them packed as
/// `ptr | len << 32`, the way the ABI returns buffers.
#[doc(hidden)]
pub fn ret(bytes: Vec<u8>) -> u64 {
    // SAFETY: single-threaded; no other reference to RESULT is alive.
    let r = unsafe { RESULT.get() };
    *r = bytes;
    if r.is_empty() {
        r.push(b' ');
    }
    (r.as_ptr() as usize as u64) | ((r.len() as u64) << 32)
}

/// Reports `e` to the host; the caller then returns its error value.
#[doc(hidden)]
pub fn fail(e: Error) {
    set_error(&e.0);
}

/// # Safety
/// `ptr..ptr+len` must be valid UTF-8 written by the host into memory from `tmv_alloc`.
#[doc(hidden)]
pub unsafe fn str_arg<'a>(ptr: *const u8, len: u32) -> &'a str {
    if len == 0 {
        return "";
    }
    core::str::from_utf8(core::slice::from_raw_parts(ptr, len as usize)).unwrap_or("")
}

#[no_mangle]
pub extern "C" fn tmv_abi() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub extern "C" fn tmv_alloc(len: u32) -> *mut u8 {
    let layout = match core::alloc::Layout::from_size_align(len.max(1) as usize, 8) {
        Ok(l) => l,
        Err(_) => return core::ptr::null_mut(),
    };
    // SAFETY: non-zero size.
    unsafe { alloc::alloc::alloc(layout) }
}

/// # Safety
/// `ptr` must come from `tmv_alloc(len)`.
#[no_mangle]
pub unsafe extern "C" fn tmv_free(ptr: *mut u8, len: u32) {
    if ptr.is_null() {
        return;
    }
    let layout = core::alloc::Layout::from_size_align_unchecked(len.max(1) as usize, 8);
    alloc::alloc::dealloc(ptr, layout)
}

/// Exports a [`Format`] implementation as `tmv_open` / `tmv_info` /
/// `tmv_page` / `tmv_close`.
#[macro_export]
macro_rules! export_format {
    ($t:ty) => {
        static __TMV_HANDLES: $crate::Global<$crate::__Vec<Option<$t>>> = $crate::Global::new($crate::__Vec::new());

        fn __tmv_get(h: i32) -> $crate::Result<&'static mut $t> {
            // SAFETY: single-threaded instance.
            let v = unsafe { __TMV_HANDLES.get() };
            v.get_mut(h as usize).and_then(|x| x.as_mut()).ok_or_else(|| $crate::Error::from("无效的文件句柄"))
        }

        /// # Safety
        /// Called by the host with a buffer from `tmv_alloc`.
        #[no_mangle]
        pub unsafe extern "C" fn tmv_open(ptr: *const u8, len: u32) -> i32 {
            let path = $crate::str_arg(ptr, len);
            match <$t as $crate::Format>::open(path) {
                Ok(v) => {
                    let hs = __TMV_HANDLES.get();
                    match hs.iter().position(|x| x.is_none()) {
                        Some(i) => {
                            hs[i] = Some(v);
                            i as i32
                        }
                        None => {
                            hs.push(Some(v));
                            (hs.len() - 1) as i32
                        }
                    }
                }
                Err(e) => {
                    $crate::fail(e);
                    -1
                }
            }
        }

        #[no_mangle]
        pub extern "C" fn tmv_info(h: i32) -> u64 {
            match __tmv_get(h).and_then(|f| <$t as $crate::Format>::info(f)) {
                Ok(i) => $crate::ret(i.to_json().into_bytes()),
                Err(e) => {
                    $crate::fail(e);
                    0
                }
            }
        }

        #[no_mangle]
        pub extern "C" fn tmv_page(h: i32, page: u32) -> u64 {
            let r = __tmv_get(h).and_then(|f| <$t as $crate::Format>::page(f, page)).map(|r| r.to_bytes());
            match r {
                Ok(b) => $crate::ret(b),
                Err(e) => {
                    $crate::fail(e);
                    0
                }
            }
        }

        #[no_mangle]
        pub extern "C" fn tmv_close(h: i32) {
            // SAFETY: single-threaded instance.
            let hs = unsafe { __TMV_HANDLES.get() };
            if let Some(x) = hs.get_mut(h as usize) {
                *x = None;
            }
        }
    };
}

/// Exports a [`Metadata`] implementation as `tmv_metadata`.
#[macro_export]
macro_rules! export_metadata {
    ($t:ty) => {
        /// # Safety
        /// Called by the host with a buffer from `tmv_alloc`.
        #[no_mangle]
        pub unsafe extern "C" fn tmv_metadata(ptr: *const u8, len: u32) -> u64 {
            let path = $crate::str_arg(ptr, len);
            match <$t as $crate::Metadata>::metadata(path) {
                Ok(Some(m)) => $crate::ret(m.to_json().into_bytes()),
                Ok(None) => $crate::ret(b"null".to_vec()),
                Err(e) => {
                    $crate::fail(e);
                    0
                }
            }
        }
    };
}

#[doc(hidden)]
pub use alloc::vec::Vec as __Vec;
