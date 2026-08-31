use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn tampered_bundle_load_returns_structured_integrity_diagnostic() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-load-failure-{suffix}"));

    let saved = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "save"])
        .arg(&root)
        .args(["--feature-id", "box-1", "--kind", "box"])
        .output()
        .expect("save process runs");
    assert!(saved.status.success());

    let manifest_path = root.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
            .expect("manifest parses");
    manifest["terminal_log_digest"] = "f".repeat(64).into();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");

    let loaded = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "load"])
        .arg(&root)
        .output()
        .expect("load process runs");
    assert_eq!(loaded.status.code(), Some(2));
    assert!(loaded.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&loaded.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "integrity_failure");
    assert_eq!(diagnostic["arg"], "log_digest_mismatch");
    assert_eq!(diagnostic["schema_version"], "threeterm.protocol/1");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn compatibility_load_failure_preserves_structured_persistence_detail() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-load-compatibility-{suffix}"));

    let saved = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "save"])
        .arg(&root)
        .args(["--feature-id", "box-1", "--kind", "box"])
        .output()
        .expect("save process runs");
    assert!(saved.status.success());

    let manifest_path = root.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
            .expect("manifest parses");
    manifest["occt_kernel_version"] = "occt/foreign".into();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");

    let loaded = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "load"])
        .arg(&root)
        .output()
        .expect("load process runs");
    assert_eq!(loaded.status.code(), Some(2));
    let diagnostic: Value = serde_json::from_slice(&loaded.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "integrity_failure");
    assert!(
        diagnostic["detail"]
            .as_str()
            .expect("diagnostic detail is a string")
            .contains("occt_kernel_version")
    );

    let _ = fs::remove_dir_all(root);
}
