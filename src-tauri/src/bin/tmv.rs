//! `tmv`: TifMrcView Pro processing from the command line (see `cli`).
use std::io::Write;

fn main() {
    let code = tifmrcview_lib::cli::main(std::env::args().skip(1).collect());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // Leave without unloading libraries: NVIDIA's Vulkan driver keeps a
    // thread running, and unloading it under that thread crashed at exit
    // (status 139 after a successful run).
    #[cfg(target_os = "linux")]
    unsafe {
        libc::_exit(code)
    }
    #[allow(unreachable_code)]
    std::process::exit(code)
}
