//! End-to-end integration test for the Project Generation identity
//! invariant on the production code path.
//!
//! Demoable behavior: chain the full MVP operation set on a project
//! through the `threeterm` binary, reload from disk, and assert the
//! Project Generation identity is byte-equal across the round-trip.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn unique_root(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-identity-{label}-{suffix}"));
    let _ = fs::remove_dir_all(&root);
    root
}

fn run(args: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(args)
        .output()
        .expect("threeterm binary runs");
    assert!(
        output.status.success(),
        "threeterm {args:?} exited with status {:?}\n  stdout: {}\n  stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn read_manifest_generation_id(root: &Path) -> String {
    let manifest_path = root.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path).expect("manifest is readable");
    let value: Value = serde_json::from_str(&raw).expect("manifest is JSON");
    value["generation_id"]
        .as_str()
        .expect("manifest.generation_id is a string")
        .to_string()
}

fn read_manifest_terminal_log_digest(root: &Path) -> String {
    let manifest_path = root.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path).expect("manifest is readable");
    let value: Value = serde_json::from_str(&raw).expect("manifest is JSON");
    value["terminal_log_digest"]
        .as_str()
        .expect("manifest.terminal_log_digest is a string")
        .to_string()
}

#[test]
fn new_project_then_save_then_load_surfaces_empty_log_identity() {
    let root = unique_root("empty");

    let new = run(&["new-project", root.to_str().expect("utf-8 path")]);
    let new_response: Value = serde_json::from_slice(&new.stdout).expect("new-project is JSON");
    let new_generation_id = new_response["generation_id"]
        .as_str()
        .expect("new-project.response.generation_id is a string")
        .to_string();
    assert_eq!(new_generation_id.len(), 64);

    run(&[
        "--machine",
        "save",
        root.to_str().expect("utf-8 path"),
        "--feature-id",
        "box-1",
        "--kind",
        "box",
    ]);
    let after_save = read_manifest_generation_id(&root);
    assert_ne!(
        after_save, new_generation_id,
        "identity advances on every accepted command"
    );

    let _loaded = run(&["--machine", "load", root.to_str().expect("utf-8 path")]);
    let after_reload = read_manifest_generation_id(&root);
    let after_reload_digest = read_manifest_terminal_log_digest(&root);
    assert_eq!(
        after_reload, after_save,
        "Project Generation identity is byte-equal after reload"
    );
    assert_eq!(
        after_reload, after_reload_digest,
        "the durable identity equals the canonical log digest"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn full_mvp_operation_set_preserves_byte_equal_identity_on_reload() {
    let root = unique_root("full-set");

    run(&["new-project", root.to_str().expect("utf-8 path")]);
    let initial = read_manifest_generation_id(&root);

    // The MVP operation set on the production code path: chain `save`
    // commands to add features, then `load` to reload from disk.
    let intents = [
        ("box", "box"),
        ("fillet-1", "fillet"),
        ("hole-1", "hole"),
        ("chamfer-1", "chamfer"),
    ];
    let mut last_identity = initial;
    for (feature_id, kind) in &intents {
        run(&[
            "--machine",
            "save",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
            "--kind",
            kind,
        ]);
        let identity = read_manifest_generation_id(&root);
        assert_ne!(
            identity, last_identity,
            "identity advances on every accepted command"
        );
        last_identity = identity;
    }

    let before_reload = read_manifest_generation_id(&root);
    let before_reload_digest = read_manifest_terminal_log_digest(&root);
    assert_eq!(
        before_reload, before_reload_digest,
        "Project Generation identity equals the canonical log digest after the full MVP operation set"
    );

    let _loaded = run(&["--machine", "load", root.to_str().expect("utf-8 path")]);
    let after_reload = read_manifest_generation_id(&root);
    assert_eq!(
        after_reload, before_reload,
        "Project Generation identity is byte-equal after reload"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn reloading_an_untouched_bundle_keeps_identity_byte_equal() {
    let root = unique_root("stable");

    run(&["new-project", root.to_str().expect("utf-8 path")]);
    let initial = read_manifest_generation_id(&root);

    for _ in 0..2 {
        let _loaded = run(&["--machine", "load", root.to_str().expect("utf-8 path")]);
        let after = read_manifest_generation_id(&root);
        assert_eq!(after, initial);
    }

    let _ = fs::remove_dir_all(root);
}
