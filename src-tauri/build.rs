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
    println!("cargo:rerun-if-env-changed=TMV_SSH_ENGINE_PACKAGE");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("ssh-engine.tar.gz");
    if let Some(path) = std::env::var_os("TMV_SSH_ENGINE_PACKAGE") {
        println!("cargo:rerun-if-changed={}",std::path::Path::new(&path).display());
        let b=std::fs::read(&path).expect("read SSH engine");
        assert!(b.starts_with(&[31,139]) && b.len()<256*1024*1024,"invalid engine archive");
        std::fs::write(out,b).unwrap();
    } else { std::fs::write(out,[]).unwrap(); }
    #[cfg(feature = "desktop")]
    tauri_build::build()
}
