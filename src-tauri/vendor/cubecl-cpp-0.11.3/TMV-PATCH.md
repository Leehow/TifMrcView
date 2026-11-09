# Preserve NAD's separate floating-point rounding

Cached upstream cubecl-cpp 0.11.3, with its original MIT/Apache-2.0 licenses.
No package version changes. Only `src/shared/base.rs` differs from upstream.
Original cached crate SHA-256:
`fd9b916049eefb3b8f793a0da10d158c5900e377c35bc32abc235b33b05d5428`.

The C++/MSL compiler unconditionally applies InstCombine, replacing separate
rounded multiply/add/subtract operations with FMA even when the Rust kernel
did not request fast math. Actual NAD original-program comparisons exposed
large multi-iteration differences after first-step one-ULP changes.

Skip this one pass only for entrypoint names beginning `tmv_nad_eed_`.
Explicit FMA operations used for compensated arithmetic remain. The companion
wgpu-hal patch disables Metal fast math only for this entrypoint prefix, so
the native compiler cannot reintroduce those contractions. Other kernels
retain the upstream optimization pipeline and Metal options.

The explicit real-GPU NAD golden test validates numerical behavior; retaining
this source or compiling it alone does not establish GPU parity or speed.
