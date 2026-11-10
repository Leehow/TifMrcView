# Bounded NAD 3D WebAssembly module

This compiles the same `src-tauri/src/nad_eed.rs` used by the desktop edition,
without copying or changing the numerical algorithm. Rayon falls back to a
single thread on wasm32; the browser host runs the module in a worker.

Build with `cargo build --locked --release --target wasm32-unknown-unknown
--manifest-path crates/denoise-wasm/Cargo.toml`. Set `CARGO_TARGET_DIR` to the
project's shared non-indexed build location. Website HTML and assets belong to
the private website repository.

ABI version 1 exports `memory`, `tmv_nad_prepare(w,h,d)` (pointer or zero),
`tmv_nad_pointer()`, `tmv_nad_length()` and `tmv_nad_run(K/SD, iterations)`
(zero on success). Fill a float32 view after preparation; refresh the view from
`memory.buffer` and the returned pointer after running. Volumes are X-fast,
at most 128×128×32. Iterations are limited to 1–20. Invalid input returns a
status without replacing the original volume. Terminate the worker to cancel
an obsolete request; the module does not access browser storage or a server.

Run `node scripts/check-denoise-wasm.mjs <module.wasm>` to compare actual WASM
outputs with the retained original-program NAD fixtures. Numerical comparison
does not establish biological validity. The Apache-2.0 license of the parent
repository applies.
