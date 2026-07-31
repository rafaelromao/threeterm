//! Subprocess integration test for the save / load round-trip.
//!
//! This is the tracer bullet for issue #235. It runs two `threeterm` binary
//! invocations against the same bundle path:
//!
//! 1. `threeterm --machine save <bundle> --feature-id box-1 --kind box` —
//!    writes the bundle, prints a JSON response with `feature_graph_hash`,
//!    `revision_hash`, and `schema_version`.
//! 2. `threeterm --machine load <bundle>` — reads the bundle, verifies
//!    integrity, prints the same JSON response.
//!
//! The acceptance criterion is byte-for-byte equality of the two
//! `feature_graph_hash` strings and the two `revision_hash` strings across
//! the two subprocesses. This proves the same canonical state and the same
//! transactional position survive a separate process reload.

use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static COUNTER: LazyLock<AtomicU64> = LazyLock::new(|| AtomicU64::new(0));

fn unique_bundle_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "threeterm-235-{}-{}-{}",
        std::process::id(),
        label,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create bundle temp dir");
    dir
}

fn run(args: &[&str]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn assert_success_json(output: &std::process::Output, what: &str) -> Value {
    assert!(
        output.status.success(),
        "{what} exit non-zero: {:?}; stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{what} stderr must be empty on success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is utf-8");
    serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("{what} stdout is parseable JSON: {err}; raw={stdout}");
    })
}

#[test]
fn save_and_load_round_trip_produces_identical_hashes() {
    let dir = unique_bundle_dir("save_load_round_trip");
    let bundle = dir.to_str().expect("bundle path is utf-8");

    let save_output = run(&[
        "--machine",
        "save",
        bundle,
        "--feature-id",
        "box-1",
        "--kind",
        "box",
    ]);
    let save_json = assert_success_json(&save_output, "save");

    let load_output = run(&["--machine", "load", bundle]);
    let load_json = assert_success_json(&load_output, "load");

    let save_graph = save_json["feature_graph_hash"]
        .as_str()
        .expect("save.feature_graph_hash is a string");
    let load_graph = load_json["feature_graph_hash"]
        .as_str()
        .expect("load.feature_graph_hash is a string");
    let save_rev = save_json["revision_hash"]
        .as_str()
        .expect("save.revision_hash is a string");
    let load_rev = load_json["revision_hash"]
        .as_str()
        .expect("load.revision_hash is a string");

    assert_eq!(save_graph.len(), 64, "feature_graph_hash is 64 lowercase hex chars");
    assert_eq!(load_graph.len(), 64, "feature_graph_hash is 64 lowercase hex chars");
    assert_eq!(save_rev.len(), 64, "revision_hash is 64 lowercase hex chars");
    assert_eq!(load_rev.len(), 64, "revision_hash is 64 lowercase hex chars");

    assert!(
        save_graph.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "feature_graph_hash is lowercase hex, got {save_graph}"
    );
    assert!(
        save_rev.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "revision_hash is lowercase hex, got {save_rev}"
    );

    assert_eq!(
        save_graph, load_graph,
        "feature_graph_hash survives the reload"
    );
    assert_eq!(save_rev, load_rev, "revision_hash survives the reload");

    assert_eq!(
        save_json["schema_version"],
        Value::from("threeterm.command.save.response/1")
    );
    assert_eq!(
        load_json["schema_version"],
        Value::from("threeterm.command.load.response/1")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_with_repeated_calls_is_idempotent_on_same_feature() {
    let dir = unique_bundle_dir("save_idempotent");
    let bundle = dir.to_str().expect("bundle path is utf-8");

    let first = run(&[
        "--machine",
        "save",
        bundle,
        "--feature-id",
        "box-1",
        "--kind",
        "box",
    ]);
    let first_json = assert_success_json(&first, "first save");

    let second = run(&[
        "--machine",
        "save",
        bundle,
        "--feature-id",
        "box-1",
        "--kind",
        "box",
    ]);
    let second_json = assert_success_json(&second, "second save");

    assert_eq!(
        first_json["feature_graph_hash"], second_json["feature_graph_hash"],
        "idempotent save keeps the same feature_graph_hash"
    );
    assert_eq!(
        first_json["revision_hash"], second_json["revision_hash"],
        "idempotent save keeps the same revision_hash"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
