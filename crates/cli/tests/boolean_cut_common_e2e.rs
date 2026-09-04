//! End-to-end subprocess tests for `threeterm --machine boolean-cut` and
//! `threeterm --machine boolean-common`.
//!
//! Drives the production CLI binary through the public surface: create
//! a fresh bundle, save two seed features, run two overlapping extrudes,
//! then run boolean-cut and boolean-common, and assert each response
//! validates against its registered schema and commits a BREP into the
//! canonical brep/ directory.
//!
//! When the OCCT worker binary is unavailable the tests fail with a
//! clear skip message so the local dev path is green; the CI
//! archlinux container installs `opencascade` so the production path
//! runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{BOOLEAN_COMMON_COMMAND_ID, BOOLEAN_CUT_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

mod common;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-bool-{label}-{}-{nanos}",
        std::process::id(),
    ))
}

fn run(bin: &str, args: &[&str]) -> std::process::Output {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "threeterm {args:?} failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn require_worker() -> bool {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "boolean_cut_common_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return false;
    }
    true
}

fn seed_overlapping_extrudes(bin: &str, root: &Path) {
    run(bin, &["new-project", root.to_str().expect("utf-8 path")]);
    for (seed, kind) in [("box-seed-1", "box"), ("box-seed-2", "box")] {
        run(
            bin,
            &[
                "--machine",
                "save",
                root.to_str().expect("utf-8 path"),
                "--feature-id",
                seed,
                "--kind",
                kind,
            ],
        );
    }
    let base_profile = serde_json::json!([[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
    let tool_profile = serde_json::json!([[5.0, 0.0], [15.0, 0.0], [15.0, 5.0], [5.0, 5.0]]);
    common::extrude_canonical(root, "box-base", base_profile, 3.0);
    common::extrude_canonical(root, "box-tool", tool_profile, 3.0);
}

fn machine_boolean(bin: &str, root: &Path, command: &str, feature_id: &str) -> Value {
    let output = Command::new(bin)
        .args([
            "--machine",
            command,
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--base",
            "box-base",
            "--tool",
            "box-tool",
        ])
        .output()
        .expect("boolean command runs");
    assert!(
        output.status.success(),
        "{command} failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

#[test]
fn boolean_cut_command_is_registered() {
    let entry = find(BOOLEAN_CUT_COMMAND_ID).expect("boolean-cut is registered");
    assert_eq!(entry.name, "boolean-cut");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.boolean-cut.response/1"
    );
}

#[test]
fn boolean_common_command_is_registered() {
    let entry = find(BOOLEAN_COMMON_COMMAND_ID).expect("boolean-common is registered");
    assert_eq!(entry.name, "boolean-common");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.boolean-common.response/1"
    );
}

#[test]
fn boolean_cut_and_common_cli_commit_distinct_solids() {
    if !require_worker() {
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("commit");
    seed_overlapping_extrudes(bin, &root);

    let cut = machine_boolean(bin, &root, "boolean-cut", "box-cut");
    let cut_entry = find(BOOLEAN_CUT_COMMAND_ID).expect("boolean-cut is registered");
    validate(&cut_entry.response_schema, &cut).expect("cut response validates against schema");
    assert_eq!(cut["status"], "ok");
    assert_eq!(cut["operation"], "boolean_cut");
    assert_eq!(cut["feature_id"], "box-cut");
    assert_eq!(
        cut["schema_version"],
        "threeterm.command.boolean-cut.response/1"
    );

    let common = machine_boolean(bin, &root, "boolean-common", "box-common");
    let common_entry = find(BOOLEAN_COMMON_COMMAND_ID).expect("boolean-common is registered");
    validate(&common_entry.response_schema, &common)
        .expect("common response validates against schema");
    assert_eq!(common["status"], "ok");
    assert_eq!(common["operation"], "boolean_common");
    assert_eq!(common["feature_id"], "box-common");
    assert_eq!(
        common["schema_version"],
        "threeterm.command.boolean-common.response/1"
    );

    // Cut and common of overlapping solids are distinct operations with
    // distinct results.
    assert_ne!(
        cut["brep_sha256"], common["brep_sha256"],
        "cut and common must commit distinct solids"
    );

    for (feature_id, parsed) in [("box-cut", &cut), ("box-common", &common)] {
        let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
        let brep_pathbuf = PathBuf::from(brep_path);
        assert!(
            brep_pathbuf.is_file() && brep_pathbuf.starts_with(root.join("brep")),
            "committed BREP must be in the canonical directory: {brep_path:?}"
        );
        assert!(
            root.join(format!("brep/{feature_id}.brep")).is_file(),
            "committed {feature_id} BREP missing"
        );
    }

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(
        loaded.revision_hash_hex(),
        common["revision_hash"],
        "bundle head matches the last committed response"
    );

    let _ = fs::remove_dir_all(root);
}
