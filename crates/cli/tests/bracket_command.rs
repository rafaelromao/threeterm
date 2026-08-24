//! Subprocess integration test for `threeterm --machine bracket`.
//!
//! Invokes the compiled `threeterm` binary via `CARGO_BIN_EXE_threeterm`
//! (set by Cargo for integration tests) and asserts the L-bracket end-to-end
//! contract: stdout reports the snapshot hashes, the on-disk bundle contains
//! the two plate features, a subsequent `--machine load` returns identical
//! hashes, and a tampered bundle surfaces a structured integrity diagnostic
//! on stderr without mutating the canonical state.

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID, find,
};
use threeterm_protocol::schema_validator::validate;

fn fresh_bundle(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-bracket-{label}-{suffix}"))
}

#[test]
fn machine_new_project_then_bracket_commits_a_reloadable_l_bracket() {
    let root = fresh_bundle("happy");
    let bin = env!("CARGO_BIN_EXE_threeterm");

    let created = Command::new(bin)
        .args(["--machine", "new-project"])
        .arg(&root)
        .output()
        .expect("new-project process runs");
    assert!(
        created.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(created.stderr.is_empty(), "stderr must be empty on success");
    let created: Value =
        serde_json::from_slice(&created.stdout).expect("response is one JSON document");
    validate(
        &find(NEW_PROJECT_COMMAND_ID)
            .expect("new-project is registered")
            .response_schema,
        &created,
    )
    .expect("new-project response validates against its registered schema");
    let created_bundle = Bundle::at(&root).open().expect("created bundle reloads");
    assert_eq!(created_bundle.manifest.revision_count, 1);
    assert!(created_bundle.transactions.is_empty());

    let saved = Command::new(bin)
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("bracket process runs");
    assert!(
        saved.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&saved.stderr)
    );
    assert!(saved.stderr.is_empty(), "stderr must be empty on success");
    let saved: Value =
        serde_json::from_slice(&saved.stdout).expect("response is one JSON document");
    validate(
        &find(BRACKET_COMMAND_ID)
            .expect("bracket is registered")
            .response_schema,
        &saved,
    )
    .expect("bracket response validates against its registered schema");

    for key in ["feature_graph_hash", "revision_hash", "schema_version"] {
        assert!(
            saved.get(key).is_some(),
            "response is missing {key:?}; got {saved}"
        );
    }
    assert_eq!(
        saved["schema_version"],
        "threeterm.command.bracket.response/1"
    );
    for key in ["feature_graph_hash", "revision_hash"] {
        let hash = saved[key].as_str().expect("hash is a string");
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }

    let transactions =
        fs::read_to_string(root.join("transactions.log")).expect("transactions.log is readable");
    assert!(transactions.contains("\"feature_id\":\"l-1-plate-vertical\""));
    assert!(transactions.contains("\"feature_id\":\"l-1-plate-horizontal\""));
    assert!(transactions.contains("\"kind\":\"plate-vertical\""));
    assert!(transactions.contains("\"kind\":\"plate-horizontal\""));

    let loaded = Command::new(bin)
        .args(["--machine", "load"])
        .arg(&root)
        .output()
        .expect("load process runs");
    assert!(
        loaded.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    let loaded: Value =
        serde_json::from_slice(&loaded.stdout).expect("response is one JSON document");
    validate(
        &find(LOAD_COMMAND_ID)
            .expect("load is registered")
            .response_schema,
        &loaded,
    )
    .expect("load response validates against its registered schema");

    assert_eq!(
        saved["feature_graph_hash"], loaded["feature_graph_hash"],
        "bracket and load must report the same feature_graph_hash"
    );
    assert_eq!(
        saved["revision_hash"], loaded["revision_hash"],
        "bracket and load must report the same revision_hash"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn machine_bracket_on_tampered_bundle_returns_integrity_diagnostic_and_preserves_state() {
    let root = fresh_bundle("tampered");

    let saved = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-1",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("bracket process runs");
    assert!(saved.status.success(), "first bracket write succeeds");

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

    let manifest_before_failure = fs::read(&manifest_path).expect("manifest reads after tampering");
    let transactions_before =
        fs::read(root.join("transactions.log")).expect("transactions log after tampering");

    let failed = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "bracket"])
        .arg(&root)
        .args([
            "--bracket-id",
            "l-2",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ])
        .output()
        .expect("bracket process runs");
    assert_eq!(
        failed.status.code(),
        Some(2),
        "bracket on tampered bundle exits 2"
    );
    assert!(
        failed.stdout.is_empty(),
        "stdout must be empty on integrity failure"
    );
    let diagnostic: Value = serde_json::from_slice(&failed.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "integrity_failure");
    assert_eq!(diagnostic["arg"], "log_digest_mismatch");
    assert_eq!(diagnostic["schema_version"], "threeterm.protocol/1");

    assert_eq!(
        fs::read(&manifest_path).expect("manifest reads after failure"),
        manifest_before_failure,
        "canonical manifest must not be mutated by a failed bracket write"
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("transactions log after failure"),
        transactions_before,
        "canonical transaction log must not be mutated by a failed bracket write"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn machine_bracket_rejects_missing_dimensions_with_structured_diagnostic() {
    let root = fresh_bundle("missing-dim");

    let failed = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(["--machine", "bracket"])
        .arg(&root)
        .args(["--bracket-id", "l-1"])
        .output()
        .expect("bracket process runs");
    assert_eq!(failed.status.code(), Some(2));
    assert!(failed.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&failed.stderr).expect("diagnostic is JSON");
    assert_eq!(diagnostic["code"], "unknown_command");
    assert!(
        diagnostic["arg"]
            .as_str()
            .unwrap_or_default()
            .contains("--length"),
        "diagnostic must name the missing flag; got {diagnostic}"
    );

    let _ = fs::remove_dir_all(root);
}
