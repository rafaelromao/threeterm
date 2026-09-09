use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_persistence::Bundle;

mod common;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-fast-native-contract-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

#[test]
fn boolean_fuse_cli_preserves_the_worker_operation_and_artifact_binding() {
    let fixture = common::install_occt_fixture();
    let project = root("boolean-fuse");
    Bundle::create(&project).expect("bundle creates");
    common::extrude_canonical_with_worker(
        &project,
        "base",
        serde_json::json!([[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]]),
        1.0,
        &fixture.worker(),
    );
    common::extrude_canonical_with_worker(
        &project,
        "tool",
        serde_json::json!([[1.0, 0.0], [3.0, 0.0], [1.0, 2.0]]),
        1.0,
        &fixture.worker(),
    );
    let source_revision = Bundle::at(&project)
        .open()
        .expect("bundle opens before fuse")
        .revision_hash_hex()
        .to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .env("THREETERM_OCCTBUILD_WORKER", fixture.path())
        .args([
            "--machine",
            "boolean-fuse",
            "--bundle",
            project.to_str().expect("project path is utf-8"),
            "--feature-id",
            "fused",
            "--base",
            "base",
            "--tool",
            "tool",
        ])
        .output()
        .expect("CLI runs");
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("response is JSON");
    assert_eq!(response["operation"], "boolean-fuse");
    assert_eq!(response["feature_id"], "fused");
    assert!(project.join("brep/fused.brep").is_file());

    let request = fixture.last_request();
    assert_eq!(request["command_id"], "boolean_fuse");
    assert_eq!(request["args"]["feature_id"], "fused");
    assert_eq!(
        request["args"]["artifact_request"]["source_revision_id"],
        source_revision
    );

    let _ = fs::remove_dir_all(project);
}
