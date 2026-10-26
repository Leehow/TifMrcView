//! Stock Tauri denies non-local sandbox IPC only while no remote ACL is granted.
use serde_json::Value;
use std::path::Path;

fn no_remote(value: &Value, source: &str) {
    match value {
        Value::Object(object) => {
            assert!(
                !object.contains_key("remote"),
                "remote capability in {source}"
            );
            for child in object.values() {
                no_remote(child, source);
            }
        }
        Value::Array(array) => {
            for child in array {
                no_remote(child, source);
            }
        }
        _ => {}
    }
}

fn check_capability_files(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_capability_files(&path);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            no_remote(&value, &path.display().to_string());
        }
    }
}

#[test]
fn sandbox_has_no_remote_capabilities_or_app_acl_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    check_capability_files(&root.join("capabilities"));
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    no_remote(
        &config["app"]["security"]["capabilities"],
        "inline config capabilities",
    );
    // Check actual build outputs too: these resolve inline/platform capabilities
    // and permission manifests, including TOML files and build.rs customization.
    let capabilities: Value =
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/capabilities.json"))).unwrap();
    no_remote(&capabilities, "compiled capabilities");
    let manifests: Value = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/acl-manifests.json"
    )))
    .unwrap();
    assert!(manifests.get("__app__").is_none(), "app ACL manifest added; review the sandbox's non-local IPC boundary before granting app permissions");
    assert_eq!(
        config["app"]["security"]["csp"]
            .as_str()
            .unwrap()
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("frame-src ")),
        Some("frame-src 'none'"),
        "srcdoc must not navigate to a local custom-protocol document"
    );
}
