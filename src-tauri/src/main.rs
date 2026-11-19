// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature="pro")]
    if std::env::args().nth(1).as_deref()==Some("gpu-worker"){std::process::exit(tifmrcview_lib::gpu_workers::worker());}
    // The Pro app is also the `tmv` command: `tifmrcview-pro tmv …`, or run
    // through a link named `tmv` (the "AI 助手" dialog makes one). Agents use
    // it to drive the open app (`tmv app …`, `tmv mcp`) or to process
    // without a window, as on a server.
    #[cfg(feature = "pro")]
    {
        let mut args: Vec<String> = std::env::args().collect();
        let named_tmv = args.first().and_then(|a| std::path::Path::new(a).file_stem()).and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("tmv"));
        if named_tmv || args.get(1).is_some_and(|a| a == "tmv") {
            #[cfg(windows)]
            unsafe {
                // A window-subsystem program has no console: print to the caller's.
                windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
            }
            args.drain(..if named_tmv { 1 } else { 2 });
            let code = tifmrcview_lib::cli::main(args);
            use std::io::Write;
            let _ = std::io::stdout().flush();
            std::process::exit(code);
        }
    }
    if let Some(status) = tifmrcview_lib::thumb::cli(std::env::args_os()) {
        std::process::exit(status);
    }
    tifmrcview_lib::run()
}
