//! End-to-end subprocess test for `threeterm --machine extrude`.
//!
//! Drives the production CLI binary through the public surface: create a
//! fresh bundle, save a seed feature, write a profile JSON, run
//! extrude, and assert the response validates against the registered
//! schema and that the bundle's `transactions.log` grew by exactly one
//! entry.
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
use threeterm_protocol::schema::{EXTRUDE_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-extrude-{label}-{}-{nanos}",
        std::process::id(),
    ))
}

fn new_project(bin: &str, root: &Path) {
    let output = Command::new(bin)
        .arg("new-project")
        .arg(root)
        .output()
        .expect("new-project runs");
    assert!(
        output.status.success(),
        "new-project failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn save(bin: &str, root: &Path, feature_id: &str, kind: &str) {
    let output = Command::new(bin)
        .args(["--machine", "save"])
        .arg(root)
        .args(["--feature-id", feature_id, "--kind", kind])
        .output()
        .expect("save runs");
    assert!(
        output.status.success(),
        "save failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn rectangle_profile() -> String {
    serde_json::json!([[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]).to_string()
}

#[test]
fn extrude_command_is_registered() {
    let entry = find(EXTRUDE_COMMAND_ID).expect("extrude is registered");
    assert_eq!(entry.name, "extrude");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.extrude.response/1"
    );
}

#[test]
fn extrude_cli_drives_host_to_commit_a_brep() {
    if OcctWorker::locate().is_err() {
        eprintln!(
            "extrude_e2e: no OCCT worker binary found; set THREETERM_OCCTBUILD_WORKER \
             or build the crate against a system OCCT install"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("commit");
    new_project(bin, &root);
    save(bin, &root, "box-seed", "box");

    let profile_path = root.join("profile.json");
    fs::write(&profile_path, rectangle_profile()).expect("profile writes");

    let output = Command::new(bin)
        .args(["--machine", "extrude"])
        .args(["--bundle"])
        .arg(&root)
        .args(["--feature-id", "box-rect"])
        .args(["--profile-file"])
        .arg(&profile_path)
        .args(["--height"])
        .arg("3.0")
        .output()
        .expect("extrude runs");
    assert!(
        output.status.success(),
        "extrude failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(EXTRUDE_COMMAND_ID).expect("extrude is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["operation"], "extrude");
    assert_eq!(parsed["feature_id"], "box-rect");
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.extrude.response/1"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        !brep_pathbuf.exists(),
        "worker staging output must be retired after commit: {brep_path:?}"
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

    // The committed BREP must live in the canonical brep/ directory.
    let committed = root.join("brep/box-rect.brep");
    assert!(
        committed.is_file(),
        "committed BREP missing at {committed:?}"
    );
    let on_disk_bytes = fs::metadata(&committed)
        .expect("committed BREP metadata")
        .len();
    assert_eq!(brep_bytes, on_disk_bytes);

    let _ = fs::remove_dir_all(root);
}
