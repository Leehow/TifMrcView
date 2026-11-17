// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature="pro")]
    if std::env::args().nth(1).as_deref()==Some("gpu-worker"){std::process::exit(tifmrcview_lib::gpu_workers::worker());}
    if let Some(status) = tifmrcview_lib::thumb::cli(std::env::args_os()) {
        std::process::exit(status);
    }
    tifmrcview_lib::run()
}
