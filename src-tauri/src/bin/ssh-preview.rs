use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Deserialize)]
struct Request {
    path: String,
    size: u32,
    modified: u64,
    bytes: u64,
}

fn run() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin().take(16_385).read_to_end(&mut input)?;
    ensure!(input.len() <= 16_384, "request too long");
    let r: Request = serde_json::from_slice(&input)?;
    ensure!((1..=256).contains(&r.size), "invalid preview size");
    let path = Path::new(&r.path);
    ensure!(path.is_absolute(), "absolute image path required");
    let check = || -> Result<()> {
        let m = path.metadata()?;
        ensure!(
            m.is_file()
                && m.len() == r.bytes
                && m.modified()?.duration_since(UNIX_EPOCH)?.as_secs() == r.modified,
            "image changed; refresh directory"
        );
        Ok(())
    };
    check()?;
    // Do not load code/plugins from the server user's environment.
    tifmrcview_lib::thumb::configure_plugins(None);
    let png = tifmrcview_lib::thumb::thumbnail_png(path, r.size)?;
    ensure!(png.len() <= 512 * 1024, "preview too large");
    check()?;
    std::io::stdout().write_all(&png).context("write preview")?;
    Ok(())
}

fn main() {
    // This process handles one image and exits; no daemon or server installation.
    #[cfg(target_os = "linux")]
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 768 << 20,
            rlim_max: 768 << 20,
        };
        libc::setrlimit(libc::RLIMIT_AS, &limit);
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(12));
        std::process::exit(124);
    });
    if let Err(e) = run() {
        eprintln!("preview: {e:#}");
        std::process::exit(1);
    }
}
