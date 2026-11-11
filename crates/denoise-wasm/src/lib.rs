//! Small bounded WebAssembly ABI around the desktop NAD kernel. The website
//! hosts this module statically and runs it in a dedicated worker. No IMOD,
//! GPU driver, network service or browser-specific numerical fork is needed.
use std::cell::RefCell;

#[path = "../../../src-tauri/src/nad_eed.rs"]
mod nad_eed;

struct Volume {
    dims: [usize; 3],
    values: Vec<f32>,
}

thread_local! {
    static VOLUME: RefCell<Volume> = const { RefCell::new(Volume { dims: [0; 3], values: Vec::new() }) };
}

#[no_mangle]
pub extern "C" fn tmv_nad_version() -> u32 {
    1
}

/// Allocate a bounded demo volume. XYZ order, X-fast float32 samples. Returns
/// zero on failure; the host refreshes its memory view after every WASM call.
#[no_mangle]
pub extern "C" fn tmv_nad_prepare(w: usize, h: usize, d: usize) -> usize {
    if w == 0 || h == 0 || d == 0 || w > 128 || h > 128 || d > 32 {
        return 0;
    }
    VOLUME.with_borrow_mut(|v| {
        let len = w * h * d;
        let mut values = Vec::new();
        if values.try_reserve_exact(len).is_err() {
            return 0;
        }
        values.resize(len, 0.0);
        v.values = values;
        v.dims = [w, h, d];
        v.values.as_ptr() as usize
    })
}

#[no_mangle]
pub extern "C" fn tmv_nad_pointer() -> usize {
    VOLUME.with_borrow(|v| v.values.as_ptr() as usize)
}

#[no_mangle]
pub extern "C" fn tmv_nad_length() -> usize {
    VOLUME.with_borrow(|v| v.values.len())
}

/// Strength is K / population SD, exactly as in denoise_nad::prepare. Status:
/// 0 success, 1 invalid parameters, 2 invalid/unprepared input, 3 kernel error.
#[no_mangle]
pub extern "C" fn tmv_nad_run(strength: f32, iterations: usize) -> u32 {
    if !strength.is_finite() || !(0.02..=5.0).contains(&strength) || !(1..=20).contains(&iterations)
    {
        return 1;
    }
    VOLUME.with_borrow_mut(|v| {
        if v.values.is_empty() || v.values.iter().any(|x| !x.is_finite()) {
            return 2;
        }
        let (mut mean, mut m2) = (0.0f64, 0.0f64);
        for (i, &value) in v.values.iter().enumerate() {
            let delta = value as f64 - mean;
            mean += delta / (i + 1) as f64;
            m2 += delta * (value as f64 - mean);
        }
        let sd = (m2 / v.values.len() as f64).max(0.0).sqrt() as f32;
        let k = (strength * sd).max(f32::MIN_POSITIVE);
        match nad_eed::diffuse(v.values.clone(), v.dims, k, iterations, &|| Ok(())) {
            Ok(result) => {
                v.values = result;
                0
            }
            Err(_) => 3,
        }
    })
}
