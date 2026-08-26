//! End-to-end subprocess test for `threeterm --machine circular-pattern`.
//!
//! Drives the production CLI binary through the public surface: create
//! a fresh bundle, save a seed feature, extrude a base solid, run
//! circular-pattern against the base feature and a rotation axis (with
//! angle_step and count), and assert the response validates against the
//! registered schema, the bundle's `feature_graph_hash` /
//! `revision_hash` advance, and the patterned BREP is committed to the
//! canonical `<bundle>/brep/<feature_id>.brep` path.
//!
//! When the OCCT worker binary is unavailable the test soft-skip with
//! an eprintln so the local dev path is green; the CI archlinux
//! container installs `opencascade` so the production path runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_occt_worker::OcctWorker;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::{CIRCULAR_PATTERN_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

mod common;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-circular-pattern-{label}-{}-{nanos}",
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
fn circular_pattern_command_is_registered() {
    let entry = find(CIRCULAR_PATTERN_COMMAND_ID).expect("circular-pattern is registered");
    assert_eq!(entry.name, "circular-pattern");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.circular-pattern.response/1"
    );
}

#[test]
fn circular_pattern_cli_drives_host_to_commit_a_patterned_brep() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "circular_pattern_e2e: no OCCT worker binary found; set THREETERM_OCCTBUILD_WORKER \
             or build the crate against a system OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("commit");
    new_project(bin, &root);
    save(bin, &root, "circular-pattern-seed", "circular-pattern");

    let rect_profile = serde_json::json!([[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]]);
    extrude(bin, &root, "circular-pattern-base", rect_profile, 2.0);

    let output = Command::new(bin)
        .args([
            "--machine",
            "circular-pattern",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "circular-pattern-1",
            "--base",
            "circular-pattern-base",
            "--axis-point",
            "1.0,1.0,0.0",
            "--axis-normal",
            "0,0,1",
            "--angle-step",
            "1.5707963267948966",
            "--count",
            "4",
        ])
        .output()
        .expect("circular-pattern runs");
    assert!(
        output.status.success(),
        "circular-pattern failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(CIRCULAR_PATTERN_COMMAND_ID).expect("circular-pattern is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "circular_pattern");
    assert_eq!(parsed["feature_id"], "circular-pattern-1");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.circular-pattern.response/1"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        brep_pathbuf.is_file() && brep_pathbuf.starts_with(root.join("brep")),
        "committed BREP must be in the canonical directory: {brep_path:?}"
    );
    let brep_sha = parsed["brep_sha256"]
        .as_str()
        .expect("brep_sha256 is a string");
    assert_eq!(brep_sha.len(), 64);
    let brep_bytes = parsed["brep_bytes"]
        .as_u64()
        .expect("brep_bytes is a number");

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(
        loaded.feature_graph_hash_hex(),
        parsed["feature_graph_hash"]
    );
    assert_eq!(loaded.revision_hash_hex(), parsed["revision_hash"]);

    let committed = root.join("brep/circular-pattern-1.brep");
    assert!(
        committed.is_file(),
        "committed patterned BREP missing at {committed:?}"
    );
    let on_disk_bytes = fs::metadata(&committed)
        .expect("committed BREP metadata")
        .len();
    assert_eq!(brep_bytes, on_disk_bytes);

    let base_brep = root.join("brep/circular-pattern-base.brep");
    let patterned_bytes = fs::read(&committed).expect("patterned BREP reads");
    let base_bytes = fs::read(&base_brep).expect("base BREP reads");
    assert_ne!(
        patterned_bytes, base_bytes,
        "patterned BREP must differ byte-for-byte from the source BREP"
    );

    let _ = fs::remove_dir_all(root);
}
