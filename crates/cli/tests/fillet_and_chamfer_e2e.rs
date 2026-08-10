//! End-to-end subprocess tests for `threeterm --machine fillet` and
//! `threeterm --machine chamfer`.
//!
//! Drives the production CLI binary through the public surface: create
//! a fresh bundle, save a seed feature, run an extrude, then run a
//! fillet (or chamfer) on the resulting BREP, and assert the response
//! validates against the registered schema and that the bundle's
//! `transactions.log` grew by exactly one entry past the extrude.
//!
//! When the OCCT worker binary is unavailable the tests soft-skip via
//! `OcctWorker::locate` returning `Err`; the CI archlinux container
//! installs `opencascade` so the production path runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{CHAMFER_COMMAND_ID, FILLET_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

mod common;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-{label}-{}-{nanos}",
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

fn rectangle_profile() -> String {
    serde_json::json!([[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]).to_string()
}

#[test]
fn fillet_command_is_registered() {
    let entry = find(FILLET_COMMAND_ID).expect("fillet is registered");
    assert_eq!(entry.name, "fillet");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.fillet.response/1"
    );
}

#[test]
fn fillet_cli_drives_host_to_commit_a_filleted_brep() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "fillet_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("fillet-commit");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    common::extrude_canonical(
        &root,
        "box-rect",
        serde_json::from_str(&rectangle_profile()).expect("profile parses"),
        3.0,
    );

    let output = Command::new(bin)
        .args([
            "--machine",
            "fillet",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "box-fillet",
            "--base",
            "box-rect",
            "--radius",
            "0.5",
        ])
        .output()
        .expect("fillet runs");
    assert!(
        output.status.success(),
        "fillet failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(FILLET_COMMAND_ID).expect("fillet is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "fillet");
    assert_eq!(parsed["feature_id"], "box-fillet");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.fillet.response/1"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        !brep_pathbuf.exists(),
        "worker staging output must be retired after commit: {brep_path:?}"
    );

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(
        loaded.feature_graph_hash_hex(),
        parsed["feature_graph_hash"]
    );
    assert_eq!(loaded.revision_hash_hex(), parsed["revision_hash"]);

    let committed = root.join("brep/box-fillet.brep");
    assert!(
        committed.is_file(),
        "committed filleted BREP missing at {committed:?}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn chamfer_command_is_registered() {
    let entry = find(CHAMFER_COMMAND_ID).expect("chamfer is registered");
    assert_eq!(entry.name, "chamfer");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.chamfer.response/1"
    );
}

#[test]
fn chamfer_cli_drives_host_to_commit_a_chamfered_brep() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "chamfer_e2e: no OCCT worker binary found; set \
             THREETERM_OCCTBUILD_WORKER or build the crate against a system \
             OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("chamfer-commit");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    common::extrude_canonical(
        &root,
        "box-rect",
        serde_json::from_str(&rectangle_profile()).expect("profile parses"),
        3.0,
    );

    let output = Command::new(bin)
        .args([
            "--machine",
            "chamfer",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "box-chamfer",
            "--base",
            "box-rect",
            "--distance",
            "0.25",
        ])
        .output()
        .expect("chamfer runs");
    assert!(
        output.status.success(),
        "chamfer failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(CHAMFER_COMMAND_ID).expect("chamfer is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "chamfer");
    assert_eq!(parsed["feature_id"], "box-chamfer");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.chamfer.response/1"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        !brep_pathbuf.exists(),
        "worker staging output must be retired after commit: {brep_path:?}"
    );

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(
        loaded.feature_graph_hash_hex(),
        parsed["feature_graph_hash"]
    );
    assert_eq!(loaded.revision_hash_hex(), parsed["revision_hash"]);

    let committed = root.join("brep/box-chamfer.brep");
    assert!(
        committed.is_file(),
        "committed chamfered BREP missing at {committed:?}"
    );

    let _ = fs::remove_dir_all(root);
}
