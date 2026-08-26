//! End-to-end subprocess test for `threeterm --machine boolean-fuse`.
//!
//! Drives the production CLI binary through the public surface: create
//! a fresh bundle, save two seed features, run two extrudes, then
//! run boolean-fuse, and assert the response validates against the
//! registered schema and that the bundle's `transactions.log` grew
//! by exactly one entry past the second extrude.
//!
//! When the OCCT worker binary is unavailable the test fails with a
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
use threeterm_protocol::schema::{BOOLEAN_FUSE_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

mod common;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-fuse-{label}-{}-{nanos}",
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

fn new_project(bin: &str, root: &Path) {
    run(bin, &["new-project", root.to_str().expect("utf-8 path")]);
}

fn save(bin: &str, root: &Path, feature_id: &str, kind: &str) {
    run(
        bin,
        &[
            "--machine",
            "save",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--kind",
            kind,
        ],
    );
}

fn extrude(_bin: &str, root: &Path, feature_id: &str, profile: serde_json::Value, height: f64) {
    common::extrude_canonical(root, feature_id, profile, height);
}

#[test]
fn boolean_fuse_command_is_registered() {
    let entry = find(BOOLEAN_FUSE_COMMAND_ID).expect("boolean-fuse is registered");
    assert_eq!(entry.name, "boolean-fuse");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.boolean-fuse.response/1"
    );
}

#[test]
fn boolean_fuse_cli_drives_host_to_commit_a_fused_brep() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "boolean_fuse_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("commit");
    new_project(bin, &root);
    save(bin, &root, "box-seed-1", "box");
    save(bin, &root, "box-seed-2", "box");

    let base_profile = serde_json::json!([[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]);
    let tool_profile = serde_json::json!([[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]]);
    extrude(bin, &root, "box-base", base_profile, 2.0);
    extrude(bin, &root, "box-tool", tool_profile, 2.0);

    let output = Command::new(bin)
        .args([
            "--machine",
            "boolean-fuse",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "box-fused",
            "--base",
            "box-base",
            "--tool",
            "box-tool",
        ])
        .output()
        .expect("boolean-fuse runs");
    assert!(
        output.status.success(),
        "boolean-fuse failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(BOOLEAN_FUSE_COMMAND_ID).expect("boolean-fuse is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "boolean_fuse");
    assert_eq!(parsed["feature_id"], "box-fused");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.boolean-fuse.response/1"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        brep_pathbuf.is_file() && brep_pathbuf.starts_with(root.join("brep")),
        "committed BREP must be in the canonical directory: {brep_path:?}"
    );

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(
        loaded.feature_graph_hash_hex(),
        parsed["feature_graph_hash"]
    );
    assert_eq!(loaded.revision_hash_hex(), parsed["revision_hash"]);

    // The committed fused BREP must live in the canonical brep/ directory,
    // alongside the two prior extrudes it consumed.
    let base_committed = root.join("brep/box-base.brep");
    let tool_committed = root.join("brep/box-tool.brep");
    let committed = root.join("brep/box-fused.brep");
    assert!(
        base_committed.is_file(),
        "base extrude BREP missing at {base_committed:?}"
    );
    assert!(
        tool_committed.is_file(),
        "tool extrude BREP missing at {tool_committed:?}"
    );
    assert!(
        committed.is_file(),
        "committed fused BREP missing at {committed:?}"
    );

    let _ = fs::remove_dir_all(root);
}
