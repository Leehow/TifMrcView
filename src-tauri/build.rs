fn main() {
    println!("cargo:rerun-if-env-changed=TMV_SSH_PREVIEW_HELPER");
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("ssh-preview-helper");
    if let Some(path) = std::env::var_os("TMV_SSH_PREVIEW_HELPER") {
        let helper = std::fs::read(&path).expect("read Linux SSH preview helper");
        assert!(
            helper.len() >= 64
                && helper.len() <= 32 * 1024 * 1024
                && &helper[..6] == b"\x7fELF\x02\x01"
                && &helper[18..20] == [62, 0],
            "expected Linux x86_64 ELF helper"
        );
        println!(
            "cargo:rerun-if-changed={}",
            std::path::Path::new(&path).display()
        );
        std::fs::copy(path, out).expect("copy Linux SSH preview helper");
    } else {
        std::fs::write(out, []).unwrap();
    }
    // Pro's Linux engine for SSH processing is downloaded on first use; the
    // app only carries its size and SHA-256. TMV_SSH_ENGINE_EMBED=1 (local
    // test builds whose engine is not on the download site) embeds it whole.
    println!("cargo:rerun-if-env-changed=TMV_SSH_ENGINE_PACKAGE");
    println!("cargo:rerun-if-env-changed=TMV_SSH_ENGINE_EMBED");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("ssh-engine.tar.gz");
    let (mut size, mut sha) = (0usize, String::new());
    let mut embed: Vec<u8> = vec![];
    if let Some(path) = std::env::var_os("TMV_SSH_ENGINE_PACKAGE") {
        println!("cargo:rerun-if-changed={}", std::path::Path::new(&path).display());
        let b = std::fs::read(&path).expect("read SSH engine");
        assert!(b.starts_with(&[31, 139]) && b.len() < 256 * 1024 * 1024, "invalid engine archive");
        use sha2::Digest;
        size = b.len();
        sha = format!("{:x}", sha2::Sha256::digest(&b));
        if std::env::var("TMV_SSH_ENGINE_EMBED").is_ok_and(|v| v == "1") {
            embed = b;
        }
    }
    std::fs::write(out, embed).unwrap();
    println!("cargo:rustc-env=TMV_SSH_ENGINE_SIZE={size}");
    println!("cargo:rustc-env=TMV_SSH_ENGINE_SHA256={sha}");
    #[cfg(feature = "desktop")]
    tauri_build::build()
}
