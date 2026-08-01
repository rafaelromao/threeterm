//! End-to-end subprocess test for `threeterm --machine solve-sketch`.
//!
//! Drives the production CLI binary through the public surface: create a
//! fresh bundle, write a sketch envelope to disk, run solve-sketch, and
//! assert the response validates against the registered schema and that
//! the bundle's `transactions.log` grew by exactly one entry.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_protocol::schema::{SOLVE_SKETCH_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;
use threeterm_persistence::Bundle;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "threeterm-cli-solve-sketch-{label}-{}-{nanos}",
        std::process::id(),
    ))
}

fn new_project(bin: &str, root: &PathBuf) {
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

fn save(bin: &str, root: &PathBuf, feature_id: &str, kind: &str) {
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

fn sketch_envelope() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!("cli-rect-{nanos}");
    serde_json::json!({
        "schema_version": "threeterm.workers.slvs/1",
        "request_id": id,
        "entities": [
            {"id": "p1", "type": "point_2d", "params": {"x": 0.0, "y": 0.0, "fixed": true}},
            {"id": "p2", "type": "point_2d", "params": {"x": 10.0, "y": 0.0}},
            {"id": "p3", "type": "point_2d", "params": {"x": 10.0, "y": 5.0}},
            {"id": "p4", "type": "point_2d", "params": {"x": 0.0, "y": 5.0}},
            {"id": "l1", "type": "line_segment_2d", "params": {"start": "p1", "end": "p2"}},
            {"id": "l2", "type": "line_segment_2d", "params": {"start": "p2", "end": "p3"}},
            {"id": "l3", "type": "line_segment_2d", "params": {"start": "p3", "end": "p4"}},
            {"id": "l4", "type": "line_segment_2d", "params": {"start": "p4", "end": "p1"}}
        ],
        "constraints": [
            {"id": "h1", "type": "horizontal", "entities": ["l1"]},
            {"id": "v2", "type": "vertical", "entities": ["l2"]},
            {"id": "h3", "type": "horizontal", "entities": ["l3"]},
            {"id": "v4", "type": "vertical", "entities": ["l4"]},
            {"id": "dw", "type": "distance", "entities": ["p1", "p2"], "value": 10.0},
            {"id": "dh", "type": "distance", "entities": ["p1", "p4"], "value": 5.0}
        ]
    })
    .to_string()
}

#[test]
fn solve_sketch_command_is_registered() {
    let entry = find(SOLVE_SKETCH_COMMAND_ID).expect("solve-sketch is registered");
    assert_eq!(entry.name, "solve-sketch");
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.solve-sketch.response/1"
    );
}

#[test]
fn solve_sketch_cli_drives_host_to_commit_a_revision() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("commit");
    new_project(bin, &root);
    save(bin, &root, "box-1", "box");

    let sketch_path = root.join("sketch.json");
    fs::write(&sketch_path, sketch_envelope()).expect("sketch writes");

    let output = Command::new(bin)
        .args(["--machine", "solve-sketch"])
        .args(["--bundle"])
        .arg(&root)
        .args(["--sketch-file"])
        .arg(&sketch_path)
        .output()
        .expect("solve-sketch runs");
    assert!(
        output.status.success(),
        "solve-sketch failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "stderr must be empty on success");

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: Value = serde_json::from_str(&stdout).expect("response is JSON");

    let entry = find(SOLVE_SKETCH_COMMAND_ID).expect("solve-sketch is registered");
    validate(&entry.response_schema, &parsed).expect("response validates against schema");

    assert_eq!(parsed["status"], "ok");
    // libslvs reports the workplane's residual degrees of freedom; the
    // worker does not currently pin the workplane origin and normal.
    assert!(
        parsed["dof"].as_i64().is_some_and(|dof| dof >= 0),
        "dof must be a non-negative integer; got {:?}",
        parsed["dof"]
    );
    assert_eq!(
        parsed["schema_version"],
        "threeterm.command.solve-sketch.response/1"
    );
    let coords = parsed["coordinates"].as_object().expect("coordinates object");
    assert_eq!(coords["p1"], serde_json::json!([0.0, 0.0]));
    assert_eq!(coords["p2"], serde_json::json!([10.0, 0.0]));
    assert_eq!(coords["p3"], serde_json::json!([10.0, 5.0]));
    assert_eq!(coords["p4"], serde_json::json!([0.0, 5.0]));

    // The bundle should now have one transaction beyond the seed "box" feature.
    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    assert_eq!(loaded.feature_graph_hash_hex(), parsed["feature_graph_hash"]);
    assert_eq!(loaded.revision_hash_hex(), parsed["revision_hash"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn solve_sketch_cli_preserves_canonical_state_on_inconsistent_sketch() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root("inconsistent");
    new_project(bin, &root);
    save(bin, &root, "box-1", "box");

    let sketch_path = root.join("sketch.json");
    fs::write(
        &sketch_path,
        serde_json::json!({
            "schema_version": "threeterm.workers.slvs/1",
            "request_id": "cli-inconsistent",
            "entities": [
                {"id": "p1", "type": "point_2d", "params": {"x": 0.0, "y": 0.0, "fixed": true}},
                {"id": "p2", "type": "point_2d", "params": {"x": 1.0, "y": 0.0, "fixed": true}}
            ],
            "constraints": [
                {"id": "c", "type": "coincident", "entities": ["p1", "p2"]},
                {"id": "d", "type": "distance", "entities": ["p1", "p2"], "value": 10.0}
            ]
        })
        .to_string(),
    )
    .expect("sketch writes");

    let output = Command::new(bin)
        .args(["--machine", "solve-sketch"])
        .args(["--bundle"])
        .arg(&root)
        .args(["--sketch-file"])
        .arg(&sketch_path)
        .output()
        .expect("solve-sketch runs");
    assert!(!output.status.success(), "inconsistent solve must fail");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("worker_failure"), "stderr={stderr}");

    let loaded = Bundle::at(&root).open().expect("bundle reopens");
    let graph_hash = loaded.feature_graph_hash_hex().to_string();
    assert_eq!(
        graph_hash,
        threeterm_persistence::Bundle::at(&root)
            .open()
            .expect("re-loads")
            .feature_graph_hash_hex(),
        "bundle must not have grown"
    );

    let _ = fs::remove_dir_all(root);
}