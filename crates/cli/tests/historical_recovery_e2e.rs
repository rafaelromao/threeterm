use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_persistence::Bundle;

fn temp_root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-historical-recovery-{suffix}"))
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

fn bracket(bin: &str, root: &Path) -> Value {
    run(
        bin,
        &[
            "--machine",
            "bracket",
            root.to_str().expect("utf-8 path"),
            "--bracket-id",
            "l-bracket",
            "--length",
            "10",
            "--width",
            "5",
            "--height",
            "3",
            "--thickness",
            "1",
        ],
    )
}

#[test]
fn historical_failure_and_named_restore_use_the_production_cli_path() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root();
    let bracket_response = bracket(bin, &root);
    let initial_history = Bundle::at(&root).open().expect("bracket reloads").history;
    assert_eq!(initial_history.active_snapshot().features.len(), 5);

    let created = run(
        bin,
        &[
            "--machine",
            "create-revision",
            root.to_str().expect("utf-8 path"),
            "--name",
            "before-edit",
        ],
    );
    assert_eq!(created["status"], "ok");
    assert!(
        created["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .any(|revision| revision["name"] == "before-edit")
    );

    let edited = run(
        bin,
        &[
            "--machine",
            "historical-edit",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "l-bracket-base",
            "--parameter",
            "length",
            "--value",
            "-1",
        ],
    );
    assert_eq!(edited["status"], "degraded");
    assert_eq!(
        edited["dirty_features"],
        serde_json::json!(["l-bracket-base", "l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(edited["evaluated_features"], serde_json::json!([]));
    assert_eq!(
        edited["blocked_features"],
        serde_json::json!(["l-bracket-bend", "l-bracket-finish"])
    );
    assert_eq!(
        edited["diagnostics"][0]["code"],
        "historical_geometry_invalid"
    );
    assert!(
        edited["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .any(|revision| revision["name"] == "before-edit")
    );

    let degraded = Bundle::at(&root).open().expect("degraded bundle reloads");
    let base = &degraded.history.active_snapshot().features["l-bracket-base"];
    assert_eq!(
        base.status,
        threeterm_domain::history::HistoryStatus::Broken
    );
    assert_eq!(
        base.last_valid_geometry_fingerprint,
        initial_history.active_snapshot().features["l-bracket-base"].geometry_fingerprint
    );
    assert!(base.geometry_fingerprint.is_none());
    let base_response = edited["features"]
        .as_array()
        .expect("feature recovery metadata")
        .iter()
        .find(|feature| feature["id"] == "l-bracket-base")
        .expect("base response metadata");
    assert_eq!(base_response["status"], "broken");
    let bend = &degraded.history.active_snapshot().features["l-bracket-bend"];
    assert!(bend.geometry_fingerprint.is_none());
    assert!(bend.last_valid_geometry_fingerprint.is_some());
    assert_eq!(
        degraded.history.active_snapshot().features["l-bracket-independent-finish"].status,
        threeterm_domain::history::HistoryStatus::CurrentValid
    );
    assert_eq!(
        bracket_response["feature_graph_hash"], edited["feature_graph_hash"],
        "history metadata must not replace the legacy feature graph"
    );

    let replay = run(
        bin,
        &[
            "--machine",
            "replay-verify",
            root.to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(replay["deterministic"], true);
    assert_eq!(replay["mismatch"], "");

    let manifest_before_failure = fs::read(root.join("manifest.json")).expect("manifest");
    let log_before_failure = fs::read(root.join("transactions.log")).expect("log");
    let diagnostic = run_failed(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "l-bracket-base",
            "--name",
            "does-not-exist",
        ],
    );
    assert_eq!(diagnostic["code"], "invalid_request");
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest"),
        manifest_before_failure
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log"),
        log_before_failure
    );

    let restored = run(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "l-bracket-base",
            "--name",
            "before-edit",
        ],
    );
    assert_eq!(restored["status"], "ok");
    assert_eq!(
        restored["active_revision"],
        initial_history.active_snapshot().revision_id
    );
    let recovered = Bundle::at(&root).open().expect("restored bundle reloads");
    assert_eq!(
        recovered.history.active_snapshot().features["l-bracket-base"].status,
        threeterm_domain::history::HistoryStatus::CurrentValid
    );

    let _ = fs::remove_dir_all(root);
}
