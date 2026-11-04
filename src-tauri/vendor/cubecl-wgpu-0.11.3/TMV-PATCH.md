# Wake readbacks when the GPU is lost

Published cubecl-wgpu 0.11.3, under its original licenses. Original crate SHA-256:
`c644cb7afff37320c3df8edf6d41ace041e9f70edf37d38a104e97fdfd5074b1`.
No dependency version is changed.

A real Metal command failure exposed a second issue after wgpu-hal began reporting
it: `Device::poll` can panic on a lost device before the buffer map callback fires.
Checking poison only *after* awaiting that callback can then wait forever.

Pending map channels are now closed when the device is lost, waking the reader so
it can return the stored GPU failure. Listener registration is synchronized with
loss notification, and completed reads release their listeners. A polling panic
also marks the device failed and wakes reads; stopping that thread no longer makes
its destructor panic.

Changed source: `src/compute/{mod.rs,device_poison.rs,poll.rs,stream.rs}` plus the
new `loss_notify.rs`. Run `node --test scripts/metal-completion.test.mjs` from the
application root for completion-state and lost-wakeup regressions.

This patch accompanies the local wgpu-hal Metal error-propagation patch. Remove
both once the upstream dependencies provide equivalent behavior and the real
readback regression passes.
