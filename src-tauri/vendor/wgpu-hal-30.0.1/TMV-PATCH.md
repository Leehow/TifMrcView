# Metal execution failure propagation

This is the published `wgpu-hal` 30.0.1 crate, retained under its original
MIT/Apache-2.0 licenses. The upstream crate SHA-256 is
`b6b7fb58561a792bc237628ba0792e332de418fefe145f13b5ed8201e6d52f58`.
Cargo patches this exact release locally; no dependency version is upgraded.

Only `src/metal/{mod.rs,device.rs,completion.rs}` differs from the upstream code.
Every submitted Metal command buffer is observed. An execution error makes the
fence fail persistently, wakes waiters and prevents successful tensor readback.
Successful later signal commands cannot clear an earlier compute failure.
Native status is also polled, since completion callbacks can arrive later.

Integration review additionally found that callback completion must not retire
an earlier pending submission. Fence polling now advances only over a completed
prefix, while still scanning later submissions for errors. Callbacks increment
a notification generation instead of publishing a fence value. Waits recheck
native status after wakeup and tolerate maintenance retiring the target between
checks, avoiding both premature retirement and a poll-to-wait lost wakeup.

The change addresses a membrane inference that survived GPU hangs and wrote
incorrect finite scores after the driver rejected subsequent commands. It does
not claim to prevent the hang or identify its triggering kernel.

The dependency-free completion-state regression runs with
`node --test scripts/metal-completion.test.mjs` from the repository root. The
application's membrane tests cover propagation of readback and launch failures.

References:
- https://developer.apple.com/documentation/metal/mtlcommandbuffer/error
- https://github.com/gfx-rs/wgpu/tree/v30.0.1/wgpu-hal

Remove this local copy when an upstream release provides equivalent error
propagation and the application readback regression passes against it.

Native NAD additionally disables Metal fast math only for passthrough MSL
source containing its unique `void tmv_nad_eed_` entrypoint prefix. This retains
separate float32 rounding required by original-program numerical regression;
all other kernels keep their existing compiler options. The corresponding
cubecl-cpp patch prevents implicit FMA before the Metal compiler sees the code.
