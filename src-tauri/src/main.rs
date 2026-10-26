// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(status) = tifmrcview_lib::thumb::cli(std::env::args_os()) {
        std::process::exit(status);
    }
    tifmrcview_lib::run()
}
