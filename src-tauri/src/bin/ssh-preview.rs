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
    #[serde(default)]
    plugin: Option<Plugin>,
}

#[derive(Deserialize)]
struct Plugin {
    root: String,
    version: String,
    files: Vec<(String, String)>,
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
    // Only files provisioned by the client for this request, never server plugins.
    if let Some(plugin) = &r.plugin {
        use sha2::{Digest, Sha256};
        ensure!(
            Path::new(&plugin.root).is_absolute() && plugin.files.len() == 3,
            "invalid plugin request"
        );
        for (name, expected) in &plugin.files {
            let relative =
                tifmrcview_lib::plugins::package::safe_rel(name).context("invalid plugin path")?;
            let file = Path::new(&plugin.root).join(relative);
            ensure!(
                file.metadata()?.len() <= 16 * 1024 * 1024,
                "plugin too large"
            );
            ensure!(
                format!("{:x}", Sha256::digest(std::fs::read(file)?)) == *expected,
                "preview plugin checksum mismatch"
            );
        }
        tifmrcview_lib::thumb::configure_plugins_version(
            Some(plugin.root.clone().into()),
            &plugin.version,
        );
    } else {
        tifmrcview_lib::thumb::configure_plugins(None);
    }
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
