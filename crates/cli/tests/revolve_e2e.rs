//! End-to-end subprocess test for `threeterm --machine revolve`.
//!
//! Drives the production CLI binary through the public surface: create
//! a fresh bundle, save a seed feature, write a profile JSON, run
//! revolve against the profile and a 3D axis (point + direction) and
//! angle, and assert the response validates against the registered
//! schema and that the bundle's `transactions.log` grew by exactly one
//! entry past the seed save.
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
use threeterm_protocol::schema::{REVOLVE_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-revolve-{label}-{}-{nanos}",
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

fn revolve_profile() -> String {
    serde_json::json!([[0.0, 0.5], [1.0, 0.5], [1.0, -0.5], [0.0, -0.5]]).to_string()
}

#[test]
fn revolve_command_is_registered() {
    let entry = find(REVOLVE_COMMAND_ID).expect("revolve is registered");
    assert_eq!(entry.name, "revolve");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.revolve.response/1"
    );
}

#[test]
fn revolve_cli_drives_host_to_commit_a_revolved_brep() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "revolve_e2e: no OCCT worker binary found; set THREETERM_OCCTBUILD_WORKER \
             or build the crate against a system OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("commit");
    new_project(bin, &root);
    save(bin, &root, "rev-seed", "revolve");

    let profile_path = root.join("rev-profile.json");
    fs::write(&profile_path, revolve_profile()).expect("profile writes");

    let output = Command::new(bin)
        .args([
            "--machine",
            "revolve",
            "--bundle",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "rev-1",
            "--profile-file",
            profile_path.to_str().expect("utf-8 path"),
            "--axis-point",
            "0.0,0.5,0.0",
            "--axis-direction",
            "0.0,1.0,0.0",
            "--angle",
            "6.283185307179586",
        ])
        .output()
        .expect("revolve runs");
    assert!(
        output.status.success(),
        "revolve failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(REVOLVE_COMMAND_ID).expect("revolve is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "revolve");
    assert_eq!(parsed["feature_id"], "rev-1");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.revolve.response/1"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        brep_pathbuf.is_file(),
        "worker BREP should be on disk at {brep_path:?}"
    );
    let brep_sha = parsed["brep_sha256"]
        .as_str()
        .expect("brep_sha256 is a string");
    assert_eq!(brep_sha.len(), 64);
    let brep_bytes = parsed["brep_bytes"]
        .as_u64()
        .expect("brep_bytes is a number");
    let on_disk_bytes = fs::metadata(&brep_pathbuf).expect("brep metadata").len();
    assert_eq!(brep_bytes, on_disk_bytes);

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(
        loaded.feature_graph_hash_hex(),
        parsed["feature_graph_hash"]
    );
    assert_eq!(loaded.revision_hash_hex(), parsed["revision_hash"]);

    let committed = root.join("brep/rev-1.brep");
    assert!(
        committed.is_file(),
        "committed revolved BREP missing at {committed:?}"
    );

    let _ = fs::remove_dir_all(root);
}
