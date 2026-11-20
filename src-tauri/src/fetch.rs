//! Downloads one large, pinned file (the SAM model, Pro's SSH engine): tries
//! each URL in turn, streams to `<dest>.part`, checks size and SHA-256, and
//! only then renames it into place, so a broken download never looks done.
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

pub async fn to_file(
    urls: &[String],
    dest: &Path,
    size: u64,
    sha256: &str,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| err(&e))?;
    }
    let part = dest.with_extension("part");
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .user_agent(format!("{}/{}", crate::edition::NAME, env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| err(&e))?;
    let mut last_err = String::from("没有下载地址");
    for url in urls {
        let attempt = async {
            let mut resp = client.get(url).send().await.map_err(|e| err(&e))?;
            if !resp.status().is_success() {
                return Err(format!("服务器返回 {}", resp.status().as_u16()));
            }
            let total = resp.content_length().unwrap_or(size);
            let mut file = std::io::BufWriter::new(std::fs::File::create(&part).map_err(|e| err(&e))?);
            let mut hash = Sha256::new();
            let (mut done, mut shown) = (0u64, 0u64);
            progress(0, total);
            while let Some(chunk) = resp.chunk().await.map_err(|e| err(&e))? {
                file.write_all(&chunk).map_err(|e| err(&e))?;
                hash.update(&chunk);
                done += chunk.len() as u64;
                if done > size {
                    return Err("下载的文件比预期大".into());
                }
                if done - shown > 1 << 20 || done == total {
                    shown = done;
                    progress(done, total);
                }
            }
            file.flush().map_err(|e| err(&e))?;
            drop(file);
            if done != size || format!("{:x}", hash.finalize()) != sha256 {
                return Err("下载的文件不完整或已损坏，请重试".into());
            }
            std::fs::rename(&part, dest).map_err(|e| err(&e))
        };
        match attempt.await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                last_err = e;
            }
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Serves `body` once per connection over plain HTTP on localhost.
    fn serve(body: Vec<u8>, times: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/engine.tar.gz", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(times) {
                let mut s = stream.unwrap();
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
            }
        });
        url
    }

    #[test]
    fn keeps_only_a_complete_matching_file_and_falls_back_to_the_next_url() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("engine").join("x.tar.gz");
        let good = b"engine bytes".to_vec();
        let sha = format!("{:x}", Sha256::digest(&good));
        let wrong = serve(b"engine bytez".to_vec(), 1);
        let right = serve(good.clone(), 1);
        // A wrong file fails and leaves nothing behind.
        let r = tauri::async_runtime::block_on(to_file(&[wrong.clone()], &dest, good.len() as u64, &sha, |_, _| {}));
        assert!(r.is_err());
        assert!(!dest.exists() && !dest.with_extension("part").exists());
        // A dead first host falls through to the next one.
        let mut seen = 0;
        let urls = ["http://127.0.0.1:1/none".to_string(), right];
        tauri::async_runtime::block_on(to_file(&urls, &dest, good.len() as u64, &sha, |done, _| seen = done)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), good);
        assert_eq!(seen, good.len() as u64);
    }
}
