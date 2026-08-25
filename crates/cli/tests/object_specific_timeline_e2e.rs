use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{TIMELINE_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-object-timeline-{suffix}"))
}

fn run(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "command {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

fn run_failed(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("diagnostic is JSON")
}

fn bracket(bin: &str, root: &Path, id: &str) {
    run(
        bin,
        &[
            "--machine",
            "bracket",
            root.to_str().expect("utf-8 path"),
            "--bracket-id",
            id,
            "--length",
            "10",
            "--width",
            "5",
            "--height",
            "3",
            "--thickness",
            "1",
        ],
    );
}

fn timeline(bin: &str, root: &Path, feature_id: &str) -> Value {
    let response = run(
        bin,
        &[
            "--machine",
            "timeline",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
        ],
    );
    let schema = &find(TIMELINE_COMMAND_ID)
        .expect("timeline is registered")
        .response_schema;
    validate(schema, &response).expect("timeline response validates");
    response
}

#[test]
fn object_specific_timeline_browsing_and_restore_use_the_production_cli_path() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root();
    bracket(bin, &root, "first");
    run(
        bin,
        &[
            "--machine",
            "create-revision",
            root.to_str().expect("utf-8 path"),
            "--name",
            "before-second",
        ],
    );
    bracket(bin, &root, "second");
    run(
        bin,
        &[
            "--machine",
            "historical-edit",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "first-base",
            "--parameter",
            "length",
            "--value",
            "12",
        ],
    );

    let first = timeline(bin, &root, "first-base");
    let first_ordinals: Vec<u64> = first["revisions"]
        .as_array()
        .expect("first revisions")
        .iter()
        .map(|entry| entry["ordinal"].as_u64().expect("ordinal"))
        .collect();
    assert_eq!(first_ordinals, [1, 2, 4]);
    assert!(
        first["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .any(|revision| revision["name"] == "before-second")
    );

    let second = timeline(bin, &root, "second-base");
    let second_revisions = second["revisions"].as_array().expect("second revisions");
    assert_eq!(
        second_revisions
            .iter()
            .map(|entry| entry["ordinal"].as_u64().expect("ordinal"))
            .collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(
        second["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .all(|revision| revision["name"] != "before-second")
    );

    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest");
    let log_before = fs::read(root.join("transactions.log")).expect("log");
    let mismatch = run_failed(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "second-base",
            "--name",
            "before-second",
        ],
    );
    assert_eq!(mismatch["code"], "invalid_request");
    assert!(
        mismatch
            .to_string()
            .contains("not present in named revision"),
        "diagnostic: {mismatch}"
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log"),
        log_before
    );

    let restored = run(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "first-base",
            "--name",
            "before-second",
        ],
    );
    assert_eq!(restored["status"], "ok");
    let loaded = Bundle::at(&root).open().expect("restored bundle opens");
    assert_eq!(
        loaded.history.active_snapshot().revision_id,
        "history-revision-1"
    );
    assert!(
        !loaded
            .history
            .active_snapshot()
            .features
            .contains_key("second-base")
    );

    let _ = fs::remove_dir_all(root);
}
