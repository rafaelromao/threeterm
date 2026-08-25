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
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::artifact::sha256_hex;
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
        "threeterm.command.extrude.response/3"
    );
}

#[test]
fn extrude_cli_promotes_a_validated_result_into_canonical_generation() {
    if OcctWorker::locate().is_err() {
        if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() {
            panic!("THREETERM_REQUIRE_OCCT is set but the OCCT worker is unavailable");
        }
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

    let prior_request = ExtrudeRequest::new(
        "prior-canonical",
        vec![(0.0, 0.0), (8.0, 0.0), (8.0, 4.0), (0.0, 4.0)],
        2.0,
    )
    .with_output_path(root.join("prior-stage"), "prior.brep")
    .with_feature_id("prior-box");
    Host::new()
        .extrude(
            &root,
            prior_request,
            &OcctWorker::locate().expect("worker locates"),
        )
        .expect("prior canonical BREP commits");
    let prior_manifest = fs::read(root.join("manifest.json")).expect("prior manifest reads");
    let prior_log = fs::read(root.join("transactions.log")).expect("prior log reads");
    let prior_brep = fs::read(root.join("brep/prior-box.brep")).expect("prior BREP reads");
    let prior_snapshot = Host::new().load(&root).expect("prior snapshot loads");

    let profile_path = root.join("profile.json");
    fs::write(&profile_path, rectangle_profile()).expect("profile writes");

    let output = Command::new(bin)
        .stdin(Stdio::null())
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
        "threeterm.command.extrude.response/3"
    );
    let brep_path = parsed["brep_path"].as_str().expect("brep_path is a string");
    let brep_pathbuf = PathBuf::from(brep_path);
    assert!(
        brep_pathbuf.is_file(),
        "canonical artifact must remain available: {brep_path:?}"
    );
    assert!(
        brep_pathbuf.starts_with(root.join("brep")),
        "promoted artifact must be canonical: {brep_path:?}"
    );
    assert_eq!(parsed["authoritative"], true);
    assert_eq!(parsed["artifact_kind"], "brep");
    assert!(
        !parsed["artifact_name"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert_eq!(
        parsed["source_snapshot"]["feature_graph_hash"],
        prior_snapshot.feature_graph_hash
    );
    assert_eq!(
        parsed["source_snapshot"]["revision_hash"],
        prior_snapshot.revision_hash
    );
    assert_ne!(
        parsed["feature_graph_hash"],
        prior_snapshot.feature_graph_hash
    );
    assert_ne!(parsed["revision_hash"], prior_snapshot.revision_hash);
    assert!(!parsed["request_id"].as_str().unwrap_or_default().is_empty());
    assert_eq!(parsed["worker_fingerprint"]["worker_kind"], "occt");
    assert_eq!(
        parsed["worker_fingerprint"]["worker_schema_version"],
        "threeterm.workers.occt/1"
    );
    assert_eq!(
        parsed["worker_fingerprint"]["protocol_schema_version"],
        "threeterm.protocol/1"
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

    let on_disk_bytes = fs::metadata(&brep_pathbuf)
        .expect("derived BREP metadata")
        .len();
    assert_eq!(brep_bytes, on_disk_bytes);
    assert_eq!(
        sha256_hex(&fs::read(&brep_pathbuf).expect("canonical BREP reads")),
        brep_sha
    );
    assert_ne!(
        fs::read(root.join("manifest.json")).expect("manifest re-reads"),
        prior_manifest
    );
    assert_ne!(
        fs::read(root.join("transactions.log")).expect("log re-reads"),
        prior_log
    );
    assert_eq!(
        fs::read(root.join("brep/prior-box.brep")).expect("prior BREP re-reads"),
        prior_brep
    );
    assert!(root.join("brep/box-rect.brep").exists());
    assert!(!root.join(".derived").exists());

    let _ = fs::remove_dir_all(root);
}
